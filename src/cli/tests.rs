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

#[cfg(feature = "client")]
#[test]
fn a_multisigs_deploys_are_where_its_nonce_puts_them_before_anyone_signs() {
    use crate::structs::code::Code;
    use crate::structs::hash::HashType;
    use crate::structs::tx::Tx;

    let account = Address::Multisig([6; 32]);
    let hash = HashType::default_chain();
    let deploy = Tx::Deploy { code: Code::Lua("return {}".to_string()) };
    let lone = crate::cli::send::landings(&deploy, &account, 4, hash);
    let batch = crate::cli::send::landings(&Tx::Batch { steps: vec![deploy.clone(), Tx::Burn { from: account, amount: crate::structs::amount::Amount::U128(1) }, deploy] }, &account, 4, hash);
    let lands = crate::cli::send::batch_deploys(&[Tx::Deploy { code: Code::Lua(String::new()) }, Tx::Deploy { code: Code::Lua(String::new()) }], &account, 4, hash);

    assert_eq!(lone, [format!("contract {}", crate::registry::contracts::contract_address(&account, 4, hash))]);
    assert_eq!(batch, [format!("step 1 contract {}", lands[0].1), format!("step 3 contract {}", lands[1].1)]);
    assert!(crate::cli::send::landings(&Tx::Burn { from: account, amount: crate::structs::amount::Amount::U128(1) }, &account, 4, hash).is_empty());
}

#[cfg(all(feature = "client", feature = "sha256"))]
#[test]
fn a_batchs_steps_show_a_deploy_by_its_engine_size_hash_and_address_not_its_source() {
    use crate::cli::multisig::{step_lines, step_text};
    use crate::structs::code::Code;
    use crate::structs::hash::HashType;
    use crate::structs::tx::Tx;

    let account = Address::Multisig([6; 32]);
    let source = format!("-- a long contract\nlocal M = {{}}\n{}return M\n", "function M.f() return 1 end\n".repeat(200));
    let burn = Tx::Burn { from: account, amount: crate::structs::amount::Amount::U128(1) };
    let batch = Tx::Batch { steps: vec![burn.clone(), Tx::Deploy { code: Code::Lua(source.clone()) }, Tx::Deploy { code: Code::Wasm(vec![0, 0x61, 0x73, 0x6d]) }] };
    let lands = crate::cli::send::batch_deploys(batch.batch_steps().unwrap(), &account, 4, HashType::default_chain());
    let lines = step_lines(&batch, &lands, false);
    let sha256 = |bytes: &[u8]| crate::structs::hex::to_hex(&crate::crypto::hasher::Hasher::digest(HashType::Sha256, bytes).to_bytes());

    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(lines[0], "a batch of 3 steps, all or nothing:");
    assert_eq!(lines[1], format!("  step 1: {}", crate::structs::json::tx(&burn)), "a step that is no deploy, as the API writes it");
    let deploy: serde_json::Value = serde_json::from_str(lines[2].trim_start_matches("  step 2: ")).unwrap();
    assert_eq!(deploy, serde_json::json!({ "type": "deploy", "engine": "lua", "code_size": source.len(), "code_hash": format!("sha256:{}", sha256(source.as_bytes())), "address": lands[0].1.to_string() }));
    assert!(lines.iter().all(|line| !line.contains("function M.f")), "no source: {lines:?}");
    assert!(lines[3].contains(&lands[1].1.to_string()) && lines[3].contains(r#""engine":"wasm""#) && lines[3].contains(r#""code_size":4"#), "{}", lines[3]);

    // With full, the code whole: Lua's source, a module's hex.
    let full = step_lines(&batch, &lands, true);
    assert!(full[2].contains("function M.f") && full[2].contains("code_hash"), "{}", full[2]);
    assert!(full[3].contains(r#""code":"0x0061736d""#), "{}", full[3]);

    // A lone deploy reads the same, and what is no batch lists no steps.
    let lone = step_text(&Tx::Deploy { code: Code::Lua(source.clone()) }, Some(&lands[0].1), false);
    assert!(!lone.contains("function M.f") && lone.contains(&lands[0].1.to_string()), "{lone}");
    assert!(step_lines(&burn, &[], false).is_empty());
}

#[cfg(feature = "client")]
#[test]
fn a_member_names_an_xpub_an_evm_address_both_or_neither() {
    let key = Keypair::from_seed([1; 32]).public().to_string();
    #[cfg(all(feature = "bitcoin", feature = "mnemonic"))]
    let xpub = {
        let seed = crate::crypto::mnemonic::Phrase::parse("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap().seed("");

        crate::crypto::bitcoin::multisig::xpub_text(&seed.multisig_account(0), crate::crypto::bitcoin::tx::Network::Main)
    };

    for line in [key.clone(), format!("{key},,0x{}", "11".repeat(20))] {
        assert!(crate::cli::send::parse_member(&line).is_ok(), "{line}");
    }

    #[cfg(all(feature = "bitcoin", feature = "mnemonic"))]
    for line in [format!("{key},{xpub}"), format!("{key},{xpub},"), format!("{key},{xpub},0x{}", "11".repeat(20))] {
        assert!(crate::cli::send::parse_member(&line).is_ok(), "{line}");
    }

    assert!(crate::cli::send::parse_member(&format!("{key},,0x12")).is_err());
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
fn a_key_shows_its_public_key_and_addresses_never_its_secret() {
    let ed = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap();
    let shown: std::collections::BTreeMap<_, _> = described(&ed).into_iter().collect();
    let address = ed.public().address().to_string();

    assert_eq!(shown["scheme"], ed.public().key_type().name());
    assert_eq!(shown["address"], address);
    // Only with a scheme whose address is its key: a build with secp256k1 alone has neither.
    #[cfg(any(feature = "ed25519", feature = "edblake"))]
    assert!(address.ends_with(&shown["public"]), "an Ed25519 key is its address: {shown:?}");

    for line in described(&ed) {
        assert!(!line.1.contains(ed.to_key_file().rsplit(':').next().unwrap()), "never the secret");
    }

    #[cfg(feature = "secp256k1")]
    {
        let secp = Keypair::generate(crate::crypto::keypair::KeyType::Secp256k1).unwrap();
        let shown: std::collections::BTreeMap<_, _> = described(&secp).into_iter().collect();

        assert_eq!(shown["public"].len(), 2 + 66, "compressed, as a federation lists it: {shown:?}");
        assert!(shown["public"].starts_with("0x02") || shown["public"].starts_with("0x03"));
        assert_eq!(shown["address"], format!("secp256k1:{}", shown["evm"]), "the same 20 bytes on EVM chains");
    }

    #[cfg(feature = "bitcoin")]
    {
        let btc = Keypair::generate(crate::crypto::keypair::KeyType::Bitcoin).unwrap();
        let shown: std::collections::BTreeMap<_, _> = described(&btc).into_iter().collect();

        assert!(shown["address"].starts_with("bc1q") && shown["testnet"].starts_with("tb1q"), "{shown:?}");
        assert_eq!(shown["address"][4..shown["address"].len() - 6].len(), shown["testnet"][4..shown["testnet"].len() - 6].len());
    }

    let dir = TempDir::new("show");
    let path = dir.0.join("shown.key");
    write_key(&path, &ed).unwrap();

    assert!(key(KeyArgs { command: KeyCommand::Show { files: vec![path.clone(), path] } }).is_ok());
    assert!(key(KeyArgs { command: KeyCommand::Show { files: vec![dir.0.join("missing.key")] } }).is_err());
}

#[cfg(feature = "client")]
#[test]
fn renouncing_the_directory_asks_for_yes_unless_told() {
    assert!(crate::cli::send::renouncing(true, &mut "".as_bytes()).is_ok(), "--yes asks nothing");
    assert!(crate::cli::send::renouncing(false, &mut "yes\n".as_bytes()).is_ok());
    assert!(crate::cli::send::renouncing(false, &mut "y\n".as_bytes()).is_err(), "yes in full");
    assert!(crate::cli::send::renouncing(false, &mut "".as_bytes()).is_err());
}

#[cfg(all(feature = "sealed-keys", feature = "mnemonic"))]
#[test]
fn a_sealed_key_file_is_written_by_keygen_and_opens_with_its_passphrase_file() {
    use clap::Parser;

    #[derive(Parser)]
    struct Keygen {
        #[command(flatten)]
        args: KeygenArgs,
    }

    let dir = TempDir::new("sealed");
    let out = dir.0.join("sealed.key");
    let out_text = out.display().to_string();
    let pass = dir.0.join("pass");
    fs::write(&pass, "correct horse\n").unwrap();
    let wrong = dir.0.join("wrong");
    fs::write(&wrong, "correct horsf").unwrap();
    let args = || Keygen::try_parse_from(["keygen", "--seal", "--out", out_text.as_str()]).unwrap().args;

    // Asked twice, they must agree, and nothing is written otherwise.
    assert!(keygen_reading(args(), &mut "correct horse\nbattery staple\n".as_bytes()).is_err());
    assert!(!out.exists());
    assert!(keygen_reading(args(), &mut "\n\n".as_bytes()).is_err(), "no passphrase: leave it plain");
    keygen_reading(args(), &mut "correct horse\r\ncorrect horse\n".as_bytes()).unwrap();

    let text = fs::read_to_string(&out).unwrap();
    assert!(sealed(&text) && text.starts_with("chainv4-sealed-key:1:"), "{text}");
    let opened = read_key_with(&out, Some(&pass)).unwrap();
    assert!(read_key_with(&out, Some(&wrong)).err().unwrap().to_string().contains("wrong passphrase"));
    assert!(read_key_with(&out, Some(&dir.0.join("missing"))).is_err());
    fs::write(dir.0.join("empty"), "\n").unwrap();
    assert!(read_key_with(&out, Some(&dir.0.join("empty"))).err().unwrap().to_string().contains("empty"));

    // Written back plain in place, and sealed again, it is the same key throughout.
    replace_key(&out, &zeroize::Zeroizing::new(opened.to_key_file())).unwrap();
    assert!(!sealed(&fs::read_to_string(&out).unwrap()));
    assert_eq!(read_key(&out).unwrap().public(), opened.public());
    replace_key(&out, &crate::crypto::sealed::seal_key_file(&opened, "battery staple").unwrap()).unwrap();
    fs::write(&pass, "battery staple").unwrap();
    assert_eq!(read_key_with(&out, Some(&pass)).unwrap().public(), opened.public());
    assert!(!dir.0.join("sealed.key.new").exists(), "nothing left beside it");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(fs::metadata(&out).unwrap().permissions().mode() & 0o777, 0o600, "still the owner's alone");
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
                genesis: None,
                eip712: false,
                max_price: None,
                tip: None,
                multisig: None,
                out: None,
                tx,
            }
        }

        fn stop(self) -> Node {
            self.stop.store(true, Ordering::Relaxed);

            self.thread.join().unwrap().0
        }
    }

    #[test]
    fn the_api_refuses_a_transaction_signed_for_another_chain_and_takes_it_signed_for_this_one() {
        use crate::structs::tx::{FeeOffer, SignedTx, Tx};

        let running = Running::start(false);
        let key = DevAuthority::dev_keypair();
        let burn = Tx::Burn { from: key.public().address(), amount: crate::structs::amount::Amount::U128(1) };
        let elsewhere = crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::default_chain(), b"another chain");
        // An offer that pays, so what is refused is refused for its signature and nothing else.
        let fee = FeeOffer { gas_limit: 1_000_000, max_price: u256::new(1_000_000_000_000), tip: u256::ZERO };
        let post = |signed: &SignedTx| {
            let mut answer = ureq::post(format!("http://{}/txs", running.api))
                .header("content-type", "application/json")
                .config()
                .http_status_as_error(false)
                .build()
                .send(serde_json::json!({ "tx": crate::structs::hex::to_hex(&wincode::serialize(signed).unwrap()) }).to_string())
                .unwrap();

            (answer.status().as_u16(), answer.body_mut().read_to_string().unwrap())
        };
        let batch = Tx::Batch { steps: vec![burn.clone(), burn.clone()] };

        for foreign in [burn, batch.clone()] {
            let (status, said) = post(&SignedTx::sign(foreign, 0, fee, &key, &elsewhere));

            assert_eq!(status, 400, "{said}");
            assert!(said.contains("signature does not match"), "{said}");
        }

        let (status, said) = post(&SignedTx::sign(batch, 0, fee, &key, &running.genesis));

        assert_eq!(status, 202, "{said}");

        let node = running.stop();

        assert_eq!(node.mempool().len(), 1, "only what was signed for this chain waits");
    }

    #[test]
    fn send_signs_for_the_chain_its_node_names_and_refuses_before_signing_one_it_was_not_told_to() {
        let running = Running::start(true);
        let burn = || TxArgs::Burn { amount: u256::new(1) };
        let other = crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::default_chain(), b"another chain").to_string();
        let refused = send(SendArgs { genesis: Some(other.clone()), ..running.args(burn()) }).unwrap_err().to_string();

        assert!(refused.contains(&running.genesis.to_string()) && refused.contains(&other) && refused.contains("nothing was signed"), "{refused}");

        // Told the chain the node is on, in whichever case and with whatever spaces around it.
        let named = format!(" {} ", running.genesis.to_string().to_uppercase().replacen("0X", "0x", 1));
        send(SendArgs { genesis: Some(named), ..running.args(burn()) }).unwrap();

        // Told none, the node names it, and what it took runs there.
        send(running.args(burn())).unwrap();

        let node = running.stop();
        let sender = DevAuthority::dev_keypair().public().address();

        assert_eq!(crate::registry::Accounts::new(&node.chain().state()).get(&sender).unwrap().nonce, 2);
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
        let dir = TempDir::new("light-state");
        let state = dir.0.join("light.bin");
        let state_text = state.display().to_string();
        let kept = |query: &[&str]| light_args(&[&["--state", state_text.as_str()][..], query].concat());

        // Kept between runs, the next carries on from it, and --fresh starts over in its place.
        light(kept(&["status"])).unwrap();
        let first = std::fs::read(&state).unwrap();
        assert!(light::restore(&state, running.genesis).is_some(), "what it kept restores for its genesis");
        assert!(light::restore(&state, crate::crypto::hasher::Hasher::digest(running.genesis.algorithm(), b"another chain")).is_none());
        light(kept(&["status"])).unwrap();
        light(kept(&["--fresh", "status"])).unwrap();
        assert!(!first.is_empty() && light::restore(&state, running.genesis).is_some());
        std::fs::write(&state, b"not a client").unwrap();
        light(kept(&["status"])).unwrap();
        assert!(light::restore(&state, running.genesis).is_some(), "a damaged one is replaced");

        light(kept(&["history", &to_text, "--limit", "5"])).unwrap();
        assert!(Light::try_parse_from(["chainv4", "history", &to_text, "--limit", "0"]).is_err(), "at least one");

        let unindexed = Running::start(true);
        let node = format!("http://{}", unindexed.api);
        let genesis = unindexed.genesis.to_string();
        let refused = light(Light::try_parse_from(["chainv4", "--node", &node, "--genesis-id", &genesis, "--state", &state_text, "--fresh", "history", &to_text]).unwrap().args);

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

    #[test]
    fn a_multisig_sends_a_batch_from_one_bundle_its_members_sign() {
        use crate::cli::multisig::{MultisigAction, MultisigArgs};
        let running = Running::start(true);
        let dir = TempDir::new("multisig-batch");
        let members: Vec<Keypair> = (41..43).map(|seed| Keypair::from_seed([seed; 32])).collect();
        let files: Vec<PathBuf> = members.iter().enumerate().map(|(index, keypair)| {
            let path = dir.0.join(format!("member{index}.key"));
            write_key(&path, keypair).unwrap();

            path
        }).collect();
        let api = format!("http://{}", running.api);
        let multisig = |key: &PathBuf, action: MultisigAction| crate::cli::multisig(MultisigArgs { node: api.clone(), tls: Default::default(), key: Some(key.clone()), timeout: 10, action });
        let members_given = members.iter().map(|keypair| crate::cli::send::parse_member(&keypair.public().to_string()).unwrap()).collect();
        send(running.args(TxArgs::CreateMultisig { members: members_given, threshold: 2 })).unwrap();
        let creator = DevAuthority::dev_keypair().public().address();
        let account = crate::structs::address::multisig_address(&creator, 0, crate::structs::hash::HashType::default_chain());

        for to in [account].into_iter().chain(members.iter().map(|keypair| keypair.public().address())) {
            send(running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(10u128.pow(18)) })).unwrap();
        }

        let (bob, carol) = (Address::Ed25519([9; 32]), Keypair::from_seed([44; 32]).public().address());
        let bundle = dir.0.join("batch.bundle");
        let step = |to: &Address, amount: &str| serde_json::json!({ "type": "transfer", "to": to.to_string(), "amount": amount }).to_string();

        send(SendArgs {
            key: Some(files[0].clone()),
            multisig: Some(account),
            out: Some(bundle.clone()),
            ..running.args(TxArgs::Batch { file: None, steps: vec![step(&bob, "10"), step(&carol, "20")] })
        })
        .unwrap();
        // It shows what it signs, and takes a typed yes.
        let read = crate::registry::multisig::MultisigBundle::from_text(&fs::read_to_string(&bundle).unwrap()).unwrap();
        assert!(crate::cli::multisig::review_bundle(&read, "Sign", false, false, &mut "no\n".as_bytes()).unwrap_err().to_string().contains("nothing is signed"));
        assert!(crate::cli::multisig::review_bundle(&read, "Sign", false, false, &mut "".as_bytes()).is_err());
        crate::cli::multisig::review_bundle(&read, "Sign", false, false, &mut "yes\n".as_bytes()).unwrap();
        crate::cli::multisig::review_bundle(&read, "Sign", true, false, &mut "".as_bytes()).unwrap();
        multisig(&files[1], MultisigAction::Sign { bundle: bundle.clone(), submit: true, yes: true, full: false }).unwrap();
        let node = running.stop();
        let held = |address: &Address| crate::registry::Accounts::new(&node.chain().state()).get(address).unwrap();

        assert_eq!((held(&bob).balance, held(&carol).balance), (u256::new(10), u256::new(20)));
        assert_eq!(held(&account).nonce, 1, "the multisig's one nonce");
    }

    #[test]
    fn a_multisig_is_created_proposed_signed_approved_and_submitted() {
        use crate::cli::multisig::{MultisigAction, MultisigArgs};
        let running = Running::start(true);
        let dir = TempDir::new("multisig");
        let members: Vec<Keypair> = (41..44).map(|seed| Keypair::from_seed([seed; 32])).collect();
        let files: Vec<PathBuf> = members.iter().enumerate().map(|(index, keypair)| {
            let path = dir.0.join(format!("member{index}.key"));
            write_key(&path, keypair).unwrap();

            path
        }).collect();
        let api = format!("http://{}", running.api);
        let multisig = |key: &PathBuf, action: MultisigAction| crate::cli::multisig(MultisigArgs { node: api.clone(), tls: Default::default(), key: Some(key.clone()), timeout: 10, action });

        // The dev key makes a 2-of-3 and funds it; each member has a little to approve with.
        let members_given = members.iter().map(|keypair| crate::cli::send::parse_member(&keypair.public().to_string()).unwrap()).collect();
        send(running.args(TxArgs::CreateMultisig { members: members_given, threshold: 2 })).unwrap();
        let creator = DevAuthority::dev_keypair().public().address();
        let account = crate::structs::address::multisig_address(&creator, 0, crate::structs::hash::HashType::default_chain());

        for to in [account].into_iter().chain(members.iter().map(|keypair| keypair.public().address())) {
            send(running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(10u128.pow(18)) })).unwrap();
        }

        // Member 0 proposes; member 1 signs, offline as far as the bundle cares, and submits.
        let bundle = dir.0.join("pay.bundle");
        let to = Address::Ed25519([9; 32]);
        let propose = |out: &PathBuf, amount: u64| send(SendArgs {
            key: Some(files[0].clone()),
            multisig: Some(account),
            out: Some(out.clone()),
            ..running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(amount.into()) })
        });
        propose(&bundle, 10).unwrap();
        let submitted_short = multisig(&files[0], MultisigAction::Submit { bundle: bundle.clone() });
        multisig(&files[1], MultisigAction::Sign { bundle: bundle.clone(), submit: true, yes: true, full: false }).unwrap();

        // The next one goes by approval on chain: member 2 approves, and that is enough with the
        // proposer's signature, so approving submits it.
        let second = dir.0.join("again.bundle");
        propose(&second, 5).unwrap();
        multisig(&files[2], MultisigAction::Approve { bundle: second.clone(), yes: true, full: false }).unwrap();
        let shown = multisig(&files[0], MultisigAction::Show { account });
        let node = running.stop();
        let balance = |address: &Address| crate::registry::Accounts::new(&node.chain().state()).get(address).unwrap().balance;

        assert!(submitted_short.unwrap_err().to_string().contains("1 of the multisig's members back it, and it needs 2"));
        assert_eq!(balance(&to), u256::new(15));
        assert_eq!(crate::registry::Accounts::new(&node.chain().state()).get(&account).unwrap().nonce, 2);
        shown.unwrap();
    }

    #[cfg(all(feature = "secp256k1", any(feature = "ed25519", feature = "edblake")))]
    #[test]
    fn an_ed25519_members_proposal_signed_and_carried_by_a_secp256k1_member_is_priced_to_run() {
        use crate::cli::multisig::{MultisigAction, MultisigArgs};
        use crate::crypto::keypair::PrivateKey;

        let running = Running::start(true);
        let dir = TempDir::new("mixed-multisig");
        // The testnet's case: a 2-of-3 of an Ed25519 key, a secp256k1 key, a byte longer, and another.
        let members = [Keypair::from_seed([51; 32]), Keypair::from_private(PrivateKey::Secp256k1([52; 32])), Keypair::from_seed([53; 32])];
        let files: Vec<PathBuf> = members.iter().enumerate().map(|(index, keypair)| {
            let path = dir.0.join(format!("member{index}.key"));
            write_key(&path, keypair).unwrap();

            path
        }).collect();
        let api = format!("http://{}", running.api);
        let multisig = |key: &PathBuf, action: MultisigAction| crate::cli::multisig(MultisigArgs { node: api.clone(), tls: Default::default(), key: Some(key.clone()), timeout: 10, action });
        let members_given = members.iter().map(|keypair| crate::cli::send::parse_member(&keypair.public().to_string()).unwrap()).collect();
        send(running.args(TxArgs::CreateMultisig { members: members_given, threshold: 2 })).unwrap();
        let creator = DevAuthority::dev_keypair().public().address();
        let account = crate::structs::address::multisig_address(&creator, 0, crate::structs::hash::HashType::default_chain());
        send(running.args(TxArgs::Transfer { to: account.into(), amount: u256::new(10u128.pow(18)) })).unwrap();

        // The Ed25519 member proposes; the secp256k1 one signs, and submits it under its own key.
        let bundle = dir.0.join("pay.bundle");
        let to = Address::Ed25519([9; 32]);
        send(SendArgs { key: Some(files[0].clone()), multisig: Some(account), out: Some(bundle.clone()), ..running.args(TxArgs::Transfer { to: to.into(), amount: u256::new(10) }) }).unwrap();
        let signed = multisig(&files[1], MultisigAction::Sign { bundle: bundle.clone(), submit: true, yes: true, full: false });
        let node = running.stop();

        signed.expect("taken, not refused under its intrinsic cost");
        assert_eq!(crate::registry::Accounts::new(&node.chain().state()).get(&to).unwrap().balance, u256::new(10));
    }

    #[cfg(all(feature = "bitcoin", feature = "mnemonic"))]
    #[test]
    fn a_multisig_s_bitcoin_side_is_described_signed_combined_and_finished() {
        use crate::cli::multisig::{BitcoinAction, MultisigAction, MultisigArgs, sign_file};
        use crate::crypto::bitcoin::multisig::{descriptor, xpub_text};
        use crate::crypto::bitcoin::psbt::{MultisigCoin, Psbt, Utxo};
        use crate::crypto::bitcoin::tx::Network;
        use crate::crypto::mnemonic::Phrase;

        let running = Running::start(true);
        let dir = TempDir::new("multisig-bitcoin");
        let seeds: Vec<_> = [
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "legal winner thank year wave sausage worth useful legal winner thank yellow",
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        ]
        .iter()
        .map(|words| Phrase::parse(words).unwrap().seed(""))
        .collect();
        let members = seeds.iter().enumerate().map(|(at, seed)| {
            let key = seed.key(crate::crypto::keypair::KeyType::Ed25519, 0).public();
            let xpub = xpub_text(&seed.multisig_account(1), Network::Test);

            crate::cli::send::parse_member(&format!("{key},{xpub},0x{}", format!("{:02x}", 0x31 + at).repeat(20))).unwrap()
        }).collect();
        send(running.args(TxArgs::CreateMultisig { members, threshold: 2 })).unwrap();
        let account = crate::structs::address::multisig_address(&DevAuthority::dev_keypair().public().address(), 0, crate::structs::hash::HashType::default_chain());
        let api = format!("http://{}", running.api);
        let run = |action: BitcoinAction| crate::cli::multisig(MultisigArgs { node: api.clone(), tls: Default::default(), key: None, timeout: 10, action: MultisigAction::Bitcoin { account, action } });

        run(BitcoinAction::Descriptor).unwrap();
        run(BitcoinAction::Address { change: true, index: 3 }).unwrap();
        let wallet = crate::cli::multisig::bitcoin_wallet(&crate::cli::send::agent(&Default::default()).unwrap(), &api, &account).unwrap();
        running.stop();

        assert_eq!((wallet.threshold, wallet.network), (2, Network::Test), "a tpub made it a testnet multisig");
        assert_eq!(descriptor(2, &wallet.xpubs, Network::Test, 0), descriptor(2, &seeds.iter().map(|seed| seed.multisig_account(1)).collect::<Vec<_>>(), Network::Test, 0));

        // Two members sign their copies, one combines them, and it finishes.
        let utxos = [Utxo { txid: [9; 32], vout: 1, coin: MultisigCoin { value: 50_000, chain: 0, index: 0 } }];
        let psbt = wallet.spend(&utxos, crate::crypto::bitcoin::tx::script_of("tb1qqqqqp399et2xygdj5xreqhjjvcmzhxw4aywxecjdzew6hylgvsesrxh6hy", Network::Test).unwrap(), 20_000, 2, 0).unwrap();
        let (first, second) = (dir.0.join("spend.psbt"), dir.0.join("other.psbt"));
        fs::write(&first, psbt.to_base64()).unwrap();
        fs::write(&second, psbt.to_base64()).unwrap();

        // Signing shows where it goes first and takes a typed yes: anything else signs nothing.
        let untouched = fs::read(&first).unwrap();
        assert!(crate::cli::multisig::review_psbt(&first, false, &mut "no\n".as_bytes()).unwrap_err().to_string().contains("nothing is signed"));
        assert!(crate::cli::multisig::review_psbt(&first, false, &mut "".as_bytes()).is_err());
        crate::cli::multisig::review_psbt(&first, false, &mut "yes\n".as_bytes()).unwrap();
        crate::cli::multisig::review_psbt(&first, true, &mut "".as_bytes()).unwrap();
        assert_eq!(fs::read(&first).unwrap(), untouched);
        let review = psbt.multisig_review().unwrap();
        assert_eq!(review.spent, 50_000);
        assert_eq!(review.outputs[0].0, "tb1qqqqqp399et2xygdj5xreqhjjvcmzhxw4aywxecjdzew6hylgvsesrxh6hy");
        assert_eq!(review.fee, 50_000 - review.outputs.iter().map(|(_, sats)| sats).sum::<u64>());

        // What cannot be read that far is refused.
        // A PSBT writes the transaction it read: another one goes in its first pair too.
        let with_output = |change: &dyn Fn(&mut crate::crypto::bitcoin::tx::TxOut)| {
            let mut changed = psbt.clone();
            change(&mut changed.tx.outputs[0]);
            let base = changed.tx.base();
            changed.global.iter_mut().find(|pair| pair.key == [crate::crypto::bitcoin::psbt::GLOBAL_UNSIGNED_TX]).unwrap().value = base;
            changed
        };
        fs::write(dir.0.join("odd.psbt"), with_output(&|output| output.script = vec![0xff; 3]).to_base64()).unwrap();
        assert!(crate::cli::multisig::review_psbt(&dir.0.join("odd.psbt"), true, &mut "".as_bytes()).unwrap_err().to_string().contains("no address"));
        fs::write(dir.0.join("greedy.psbt"), with_output(&|output| output.value = 60_000).to_base64()).unwrap();
        assert!(crate::cli::multisig::review_psbt(&dir.0.join("greedy.psbt"), true, &mut "".as_bytes()).unwrap_err().to_string().contains("more than"));
        let mut blind = psbt.clone();
        blind.inputs[0].retain(|pair| pair.key[0] != crate::crypto::bitcoin::psbt::IN_WITNESS_UTXO);
        fs::write(dir.0.join("blind.psbt"), blind.to_base64()).unwrap();
        assert!(crate::cli::multisig::review_psbt(&dir.0.join("blind.psbt"), true, &mut "".as_bytes()).unwrap_err().to_string().contains("says nothing"));

        sign_file(&first, &seeds[0]).unwrap();
        sign_file(&second, &seeds[2]).unwrap();

        assert!(Psbt::from_base64(&fs::read_to_string(&first).unwrap()).unwrap().finish_multisig().is_err(), "one of two");
        // Combining and finishing need no node, only the files.
        run(BitcoinAction::Combine { psbt: first.clone(), others: vec![second] }).unwrap();
        run(BitcoinAction::Finish { psbt: first.clone(), esplora: None }).unwrap();

        assert_eq!(Psbt::from_base64(&fs::read_to_string(&first).unwrap()).unwrap().finish_multisig().unwrap().inputs[0].witness.len(), 4);
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    #[test]
    fn vote_change_and_clear_vote_go_through_governance() {
        use crate::governance::GovernanceConfig;

        let dev = DevAuthority::dev_keypair();
        let council = GovernanceConfig::Council {
            members: vec![dev.public().address()],
            threshold: 1,
            approval_epochs: 64,
            quorum_permille: 0,
        };
        let running = Running::launch_governed(true, false, Some(FinalityConfig::Ffg(crate::chain::FfgConfig::dev(dev.public()))), Some(council), |node| node);
        let staked = r#"{"type":"staked","slot_ms":2000,"schedule":"draw"}"#.to_string();
        let windowed = r#"{"type":"pow","hash":"blake3","target_block_ms":2000,"initial_difficulty":"1","min_difficulty":"1","window":1}"#.to_string();

        // A window stays genesis's; nothing to withdraw before approving; the council of one
        // decides at once, so there is nothing to withdraw after either.
        let bootnode = "/ip4/203.0.113.7/udp/4001/quic-v1".to_string();
        let kept = send(running.args(TxArgs::VoteChange { vote: crate::governance::ChangeVote::Yes, change: None, governance: None, consensus: Some(windowed), bootnodes: None, name_prices: None, collections: None, federation: None, directory: None, fees: None, reward: None, limits: None, finality: None, switch_finality: None, switch_epoch: None, upgrade: None, upgrade_from: None, chain_name: None, lock: None, version: None }));
        let nothing = send(running.args(TxArgs::ClearVote { kind: None, change: None, version: None }));
        let approved = send(running.args(TxArgs::VoteChange { vote: crate::governance::ChangeVote::Yes, change: None, governance: None, consensus: Some(staked), bootnodes: None, name_prices: None, collections: None, federation: None, directory: None, fees: None, reward: None, limits: None, finality: None, switch_finality: None, switch_epoch: None, upgrade: None, upgrade_from: None, chain_name: None, lock: None, version: None }));
        let decided = send(running.args(TxArgs::ClearVote { kind: None, change: None, version: None }));
        // Production by stake needs FFG, so finality cannot switch away from it under staked.
        let authority = crate::cli::genesis::finality_json(&FinalityConfig::Authority(crate::chain::AuthorityConfig::dev(dev.public()))).to_string();
        let switched = send(running.args(TxArgs::VoteChange { vote: crate::governance::ChangeVote::Yes, change: None, governance: None, consensus: None, bootnodes: None, name_prices: None, collections: None, federation: None, directory: None, fees: None, reward: None, limits: None, finality: None, switch_finality: Some(authority), switch_epoch: Some(1_000), upgrade: None, upgrade_from: None, chain_name: None, lock: None, version: None }));
        let unreadable = send(running.args(TxArgs::VoteChange { vote: crate::governance::ChangeVote::Yes, change: None, governance: None, consensus: None, bootnodes: Some(vec!["203.0.113.7:4001".to_string()]), name_prices: None, collections: None, federation: None, directory: None, fees: None, reward: None, limits: None, finality: None, switch_finality: None, switch_epoch: None, upgrade: None, upgrade_from: None, chain_name: None, lock: None, version: None }));
        let booted = send(running.args(TxArgs::VoteChange { vote: crate::governance::ChangeVote::Yes, change: None, governance: None, consensus: None, bootnodes: Some(vec![bootnode.clone()]), name_prices: None, collections: None, federation: None, directory: None, fees: None, reward: None, limits: None, finality: None, switch_finality: None, switch_epoch: None, upgrade: None, upgrade_from: None, chain_name: None, lock: None, version: None }));
        let governance = crate::cli::send::ok(crate::cli::send::agent(&Default::default()).unwrap().get(format!("http://{}/governance", running.api)).call()).unwrap();
        let node = running.stop();

        assert!(unreadable.unwrap_err().to_string().contains("each bootnode is a libp2p address"));
        booted.unwrap();
        assert_eq!(node.chain().bootnodes().unwrap(), std::slice::from_ref(&bootnode), "decided at once");
        assert_eq!(governance["bootnodes"], serde_json::json!([bootnode]));

        assert!(kept.unwrap_err().to_string().contains("window is at least 2"), "a change may name a window, but not one no chain could retarget over");
        let nothing = nothing.unwrap_err().to_string();
        let decided = decided.unwrap_err().to_string();

        assert!(nothing.contains("no vote"), "{nothing}");
        approved.unwrap();
        assert!(decided.contains("no vote"), "{decided}");
        let switched = switched.unwrap_err().to_string();

        assert!(switched.contains("yet to be fixed") || switched.contains("needs finality of type ffg"), "{switched}");
        assert_eq!(governance["version"], 2, "the consensus, then the bootnodes");
        assert_eq!(governance["decided"]["type"], "staked");
        assert!(node.chain().tip().height >= 2);
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-council"))]
    #[test]
    fn vote_change_refuses_what_a_lock_holds_before_it_sends() {
        use crate::governance::GovernanceConfig;

        let dev = DevAuthority::dev_keypair();
        let council = GovernanceConfig::Council { members: vec![dev.public().address()], threshold: 1, approval_epochs: 64, quorum_permille: 0 };
        let finality = Some(FinalityConfig::Ffg(crate::chain::FfgConfig::dev(dev.public())));
        let running = Running::launch_genesis(true, false, |genesis| Genesis { finality, governance: Some(council), locked: vec!["reward.initial".to_string()], ..genesis }, |node| node, Default::default());
        let vote = |change: Option<&str>, lock: Option<&str>| TxArgs::VoteChange {
            vote: crate::governance::ChangeVote::Yes,
            change: change.map(str::to_string),
            governance: None,
            consensus: None,
            bootnodes: None,
            name_prices: None,
            collections: None,
            federation: None,
            directory: None,
            fees: None,
            reward: None,
            limits: None,
            finality: None,
            switch_finality: None,
            switch_epoch: None,
            upgrade: None,
            upgrade_from: None,
            chain_name: None,
            lock: lock.map(|paths| paths.split(',').map(str::to_string).collect()),
            version: None,
        };

        let raised = send(running.args(vote(Some(r#"{"reward":{"initial":"5"}}"#), None)));
        let halving = send(running.args(vote(Some(r#"{"reward":{"halving_interval":1000}}"#), None)));
        let locked = send(running.args(vote(None, Some("dex"))));
        let under = send(running.args(vote(None, Some("dex.fee_bps"))));
        let dex = send(running.args(vote(Some(r#"{"dex":{"fee_bps":50}}"#), None)));
        let nothing = send(running.args(vote(None, Some("dex.nothing"))));
        let governance = crate::cli::send::ok(crate::cli::send::agent(&Default::default()).unwrap().get(format!("http://{}/governance", running.api)).call()).unwrap();
        let node = running.stop();

        assert_eq!(raised.unwrap_err().to_string(), "reward.initial is locked");
        halving.unwrap();
        locked.unwrap();
        assert_eq!(under.unwrap_err().to_string(), "dex.fee_bps is locked already, under dex");
        assert_eq!(dex.unwrap_err().to_string(), "dex is locked");
        assert!(nothing.unwrap_err().to_string().contains("names nothing in dex"));
        assert_eq!(governance["locks"], serde_json::json!(["dex", "reward.initial"]));
        assert_eq!(governance["in_force"]["locks"]["value"], serde_json::json!(["dex", "reward.initial"]));
        assert_eq!(governance["in_force"]["reward"]["value"]["halving_interval"], 1000);

        // What was refused was never sent: the account sent the two that went, and nothing else.
        assert_eq!(crate::registry::Accounts::new(&node.chain().state()).get(&dev.public().address()).unwrap().nonce, 2);
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

    #[test]
    fn a_batch_goes_out_from_a_file_of_steps_all_or_nothing_and_says_what_each_did() {
        use crate::structs::tx::{Tx, batch_deploy_nonce};

        let dir = TempDir::new("batch");
        let source = dir.0.join("counter.lua");
        fs::write(&source, "local M = {}\n\nfunction M.bump(by)\n    local count = tonumber(storage.get(\"count\") or \"0\") + by\n    storage.set(\"count\", tostring(count))\n    return count\nend\n\nreturn M\n").unwrap();
        let running = Running::start(true);
        let me = DevAuthority::dev_keypair().public().address();
        let hash = crate::structs::hash::HashType::default_chain();
        let (first, second) = (deployed_at(&me, 0, hash), deployed_at(&me, batch_deploy_nonce(0, 1), hash));
        let bob = Address::Ed25519([9; 32]);
        let file = dir.0.join("steps.json");
        let steps = serde_json::json!([
            { "type": "deploy", "lua": source.display().to_string() },
            { "type": "call", "to": first.to_string(), "function": "bump", "args": ["i64:2"] },
            { "type": "deploy", "lua": source.display().to_string() },
            { "type": "transfer", "to": bob.to_string(), "amount": "5" },
        ]);
        fs::write(&file, steps.to_string()).unwrap();
        send(running.args(TxArgs::Batch { file: Some(file), steps: Vec::new() })).unwrap();

        // One that a dry run says would fail goes out only when told to, and undoes itself then.
        let step = |text: &str| TxArgs::Batch { file: None, steps: vec![serde_json::json!({ "type": "transfer", "to": bob.to_string(), "amount": "1" }).to_string(), text.to_string()] };
        let failing = r#"{"type":"burn","amount":"115792089237316195423570985008687907853269984665640564039457584007913129639935"}"#;
        let refused = send(running.args(step(failing)));
        let anyway = send(SendArgs { anyway: true, ..running.args(step(failing)) });

        // A step no batch holds, one misspelled, none at all, and a typed signature, named at once.
        let not_held = send(running.args(step(r#"{"type":"mint","to":"x","amount":"1"}"#)));
        let misspelled = send(running.args(step(r#"{"type":"burn","amount":"1","amout":"2"}"#)));
        let empty = send(running.args(TxArgs::Batch { file: None, steps: Vec::new() }));
        let typed = send(SendArgs { eip712: true, ..running.args(step(r#"{"type":"burn","amount":"1"}"#)) });
        let node = running.stop();

        assert!(refused.unwrap_err().to_string().contains("step 2 failed, undoing every step"));
        assert!(anyway.unwrap_err().to_string().contains("the transaction failed: step 2 failed"));
        assert!(not_held.unwrap_err().to_string().contains("not mint"));
        assert!(misspelled.unwrap_err().to_string().contains("no amout"));
        assert!(empty.unwrap_err().to_string().contains("needs a step"));
        assert!(typed.unwrap_err().to_string().contains("EIP-712"));

        let batches: Vec<crate::structs::tx::SignedTx> = (1..=node.chain().tip().height).flat_map(|height| node.chain().block_at(height).unwrap().unwrap().txs).filter(|tx| matches!(tx.tx, Tx::Batch { .. })).collect();

        assert_eq!(batches.len(), 2, "the batch, and the one sent anyway");
        assert!(matches!(&batches[0].tx, Tx::Batch { steps } if steps.len() == 4));

        let state = node.chain().state();
        let accounts = crate::registry::Accounts::new(&state);

        assert_eq!(accounts.get(&bob).unwrap().balance, u256::new(5), "the first batch's, and nothing of the second");
        assert!(crate::registry::Contracts::new(&std::sync::Arc::new(node.chain().state())).get(&second).unwrap().is_some());
        assert!(crate::registry::Contracts::new(&std::sync::Arc::new(node.chain().state())).get(&first).unwrap().is_some());
    }

    #[cfg(feature = "evm")]
    #[test]
    fn a_step_reads_as_its_send_command_would_and_names_what_is_wrong() {
        use crate::structs::tx::Tx;

        let me = crate::crypto::keypair::Keypair::from_seed([1; 32]).public().address();
        let agent = ureq::Agent::new_with_defaults();
        let made = |steps: &[&str]| {
            let steps: Vec<String> = steps.iter().map(|step| step.to_string()).collect();

            super::batch::batch(&steps, None, me, &agent, "http://127.0.0.1:1")
        };
        let to = crate::crypto::keypair::Keypair::from_seed([2; 32]).public().address().to_string();
        let evm = format!("0x{}", "cd".repeat(20));
        let path = format!("0x{}", "00".repeat(32));
        let tx = made(&[
            &format!(r#"{{"type":"transfer","to":"{to}","amount":"5"}}"#),
            r#"{"type":"burn","amount":7}"#,
            &format!(r#"{{"type":"evm_call","to":"{evm}","data":"0x01020304","value":"3"}}"#),
            &format!(r#"{{"type":"claim","height":9,"to":"{to}","earned":"100","index":2,"path":["{path}"]}}"#),
        ])
        .unwrap();
        let Tx::Batch { steps } = tx else { panic!("{tx:?}") };

        assert!(matches!(&steps[0], Tx::Transfer { from, amount, .. } if *from == me && amount.to_u256() == u256::new(5)), "every step from the sender");
        assert!(matches!(&steps[1], Tx::Burn { from, .. } if *from == me));
        assert!(matches!(&steps[2], Tx::EvmCall { to: Address::Evm(_), input, .. } if input.len() == 4));
        assert!(matches!(&steps[3], Tx::Claim { height: 9, index: 2, path, .. } if path.len() == 1));

        assert!(made(&[r#"{"type":"transfer","amount":"5"}"#]).unwrap_err().to_string().starts_with("step 1: a transfer step needs to"));
        assert!(made(&["not json"]).unwrap_err().to_string().contains("not JSON"));
        assert!(made(&[r#"{"amount":"5"}"#]).unwrap_err().to_string().contains("needs a type"));
        assert!(made(&[r#"{"type":"burn","amount":"0"}"#]).unwrap_err().to_string().contains("greater than zero"), "as the chain would say it");
        assert!(made(&vec![r#"{"type":"burn","amount":"1"}"#; 33]).unwrap_err().to_string().contains("at most 32"));
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
        let probe = SignedTx::sign(Tx::EvmCall { to: counter, value: Amount::U128(0), input: super::super::evm::call("count()", &[]).unwrap() }, 3, FeeOffer::NONE, &deployer, &running.genesis);
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

    #[cfg(feature = "updates")]
    #[test]
    fn update_takes_the_binary_a_threshold_signed_from_the_release_a_node_relays() {
        use crate::cli::update::update_reading;
        use crate::cli::UpdateArgs;
        use crate::release::{Product, Release, ReleaseFile, ReleaseKeys, SignedRelease, TARGET, sha256};
        use crate::structs::version::Version;

        let dir = std::env::temp_dir().join(format!("chainv4-update-{}", std::process::id()));
        let served = dir.join("served");
        std::fs::create_dir_all(&served).unwrap();
        let next = b"the next chainv4".to_vec();
        std::fs::write(served.join("chainv4-next"), &next).unwrap();
        let path = dir.join("chainv4");
        std::fs::write(&path, b"this chainv4").unwrap();
        let signers: Vec<Keypair> = (1..=3).map(|seed| Keypair::from_seed([seed; 32])).collect();
        let keys = ReleaseKeys { threshold: 2, keys: signers.iter().map(Keypair::public).collect(), updates: None, floor: 0 };
        let releases = Arc::new(crate::node::Releases::new(keys.clone(), None));
        // The API serves the directory from /, as it would an explorer, so the binary downloads from it.
        let options = crate::api::ApiOptions { explorer: Some(served.clone()), releases: Some(releases.clone()), ..Default::default() };
        let running = Running::launch_serving(false, false, None, None, |node| node, options);
        let ours = Version::ours();
        let newest = Version::new(ours.major, ours.minor + 1, 0);
        let file = ReleaseFile { target: TARGET.to_string(), url: format!("http://{}/chainv4-next", running.api), sha256: sha256(&next), size: next.len() as u64 };
        let product = Product { name: "chainv4".to_string(), version: newest, least: ours, notes: "the next one".to_string(), files: vec![file] };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let dated = |issued: u64, expires: u64| {
            let mut signed = SignedRelease { release: Release { issued, expires, products: vec![product.clone()], breaking: Vec::new() }, signatures: Vec::new() };
            signed.sign(&signers[0]);
            signed
        };
        let mut signed = dated(now - 100, now + 1_000);

        assert!(releases.offer(signed.clone(), now * 1_000).is_err(), "one of the three keys is too few");

        signed.sign(&signers[2]);
        assert_eq!(releases.offer(signed.clone(), now * 1_000), Ok(true));
        assert_eq!(releases.offer(signed.clone(), now * 1_000), Ok(false), "the same release again is no newer");

        let release: serde_json::Value = serde_json::from_str(&ureq::get(format!("http://{}/release", running.api)).call().unwrap().body_mut().read_to_string().unwrap()).unwrap();

        assert_eq!(release["standing"], "behind");
        assert_eq!(release["newest"], newest.to_string());

        let seen = dir.join("seen.json");
        let args = || UpdateArgs { node: Some(format!("http://{}", running.api)), updates: None, proxy: None, check: false, yes: false, seen: Some(seen.clone()) };
        let stricter = ReleaseKeys { threshold: 3, ..keys.clone() };
        let refused = update_reading(args(), &stricter, &path, &mut "yes\n".as_bytes()).unwrap_err();

        assert!(refused.to_string().contains("2 of the release keys signed it, and it takes 3"), "{refused}");
        assert!(update_reading(args(), &keys, &path, &mut "no\n".as_bytes()).is_err(), "only yes replaces it");
        assert_eq!(std::fs::read(&path).unwrap(), b"this chainv4");

        update_reading(args(), &keys, &path, &mut "yes\n".as_bytes()).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), next);

        // It kept what it saw: a host serving an older one takes it back to nothing. Here the node's
        // is the older, as the machine saw a newer one, and the newer one's file is what it installs.
        std::fs::write(&path, b"this chainv4").unwrap();
        let mut newer = dated(now - 50, now + 1_000);
        newer.sign(&signers[2]);
        crate::release::write_kept(&seen, &newer).unwrap();
        update_reading(args(), &keys, &path, &mut "yes\n".as_bytes()).unwrap();
        assert_eq!(crate::release::kept_release(&std::fs::read_to_string(&seen).unwrap(), &keys).unwrap().release.issued, now - 50, "not gone back");

        // Past its expires, the newest seen counts for nothing: this says why, and replaces nothing.
        std::fs::write(&path, b"this chainv4").unwrap();
        let mut lapsed = dated(now - 50, now - 1);
        lapsed.sign(&signers[2]);
        crate::release::write_kept(&seen, &lapsed).unwrap();
        let refused = update_reading(args(), &keys, &path, &mut "yes\n".as_bytes()).unwrap_err().to_string();
        assert!(refused.contains("expired") && refused.contains("clock"), "{refused}");
        assert_eq!(std::fs::read(&path).unwrap(), b"this chainv4");

        // Nor is one older than the floor built in taken, with nothing seen.
        std::fs::remove_file(&seen).unwrap();
        let later = ReleaseKeys { floor: now, ..keys.clone() };
        assert!(update_reading(args(), &later, &path, &mut "yes\n".as_bytes()).unwrap_err().to_string().contains("before the oldest"));
        assert_eq!(std::fs::read(&path).unwrap(), b"this chainv4");

        // An expired one a node relays is refused, and nothing without a file to keep it stops that.
        let quiet = Arc::new(crate::node::Releases::new(keys.clone(), None));
        assert!(quiet.offer(lapsed, now * 1_000).is_err());
        running.stop();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A worker on an open port, end to end: TLS as nginx gives it, the node's real WebSocket relay and
    /// mining port behind that, on one CPU thread. Asked over its API, it goes over wss:// without being
    /// told, and blocks it finds pay what it said.
    #[test]
    fn a_worker_mines_on_an_open_port_over_wss_found_from_the_nodes_api_with_its_certificate_checked() {
        use crate::consensus::PowConfig;
        use crate::mining::pool::{MiningPort, OPEN_TOKEN};
        use crate::mining::worker::{Devices, Worker, WorkerConfig};
        use crate::mining::wss::tests::{CERT, accept_one, tls_listener};
        use crate::node::{Mining, Producing};
        use crate::structs::hash::HashType;
        use std::io::{Read, Write};

        let listen = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let payout = Address::Ed25519([4; 32]);
        let pow = |genesis: Genesis| Genesis {
            consensus: ConsensusConfig::ProofOfWork(PowConfig { hash: HashType::default_chain(), target_block_ms: 1_000, initial_difficulty: 64, min_difficulty: 64, tuning: None, window: None }),
            ..genesis
        };
        let mining = move |node: Node| {
            node.producing(Producing {
                mine: Some(Mining {
                    reward_to: Address::Ed25519([9; 32]),
                    threads: 0,
                    gpu: false,
                    gpus: None,
                    port: Some(MiningPort { listen, token: OPEN_TOKEN.to_vec(), protocol: "chainv4".to_string(), #[cfg(feature = "pool")] pool: None, #[cfg(feature = "updates")] releases: None, solo: true, limits: crate::mining::pool::PortLimits::DEFAULT }),
                }),
                sign: None,
            })
        };
        // It publishes its port elsewhere, as a real one does: on this machine it could go over TCP.
        let options = crate::api::ApiOptions { mining_public: Some("pool.invalid:4002".to_string()), ..Default::default() };
        let running = Running::launch_genesis(false, false, pow, mining, options);

        // In front of the node's API, TLS, as nginx would have it: each connection passed on as it is.
        let (front, config) = tls_listener();
        let port = front.local_addr().unwrap().port();
        let api = running.api;

        std::thread::spawn(move || {
            while let Ok(mut tls) = accept_one(&front, &config) {
                let mut behind = std::net::TcpStream::connect(api).unwrap();
                behind.set_read_timeout(Some(std::time::Duration::from_millis(5))).unwrap();
                tls.sock.set_read_timeout(Some(std::time::Duration::from_millis(5))).unwrap();

                std::thread::spawn(move || {
                    let mut buffer = [0; 16 * 1024];

                    loop {
                        match tls.read(&mut buffer) {
                            Ok(0) => return,
                            Ok(size) => {
                                if behind.write_all(&buffer[..size]).is_err() {
                                    return;
                                }
                            }
                            Err(err) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                            Err(_) => return,
                        }

                        match behind.read(&mut buffer) {
                            Ok(0) => return,
                            Ok(size) => {
                                if tls.write_all(&buffer[..size]).and_then(|()| tls.flush()).is_err() {
                                    return;
                                }
                            }
                            Err(err) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                            Err(_) => return,
                        }
                    }
                });
            }
        });

        let dir = TempDir::new("worker-wss");
        let ca = dir.0.join("node.pem");
        fs::write(&ca, CERT).unwrap();
        let about: serde_json::Value = serde_json::from_str(&ureq::get(format!("http://{api}/mining")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap();
        assert_eq!((about["open"].as_bool(), about["ws"].as_str()), (Some(true), Some("/mining/ws")), "what a worker needs of it");

        let (node, token, protocol) = crate::cli::worker::route_for_tests(&format!("https://localhost:{port}"), None, false, Some(CERT)).unwrap();
        assert_eq!(node, format!("wss://localhost:{port}/mining/ws"));

        let worker = Worker::start(WorkerConfig {
            node,
            token,
            name: "rig".to_string(),
            protocol: protocol.unwrap(),
            devices: Devices::Cpu(1),
            proxy: None,
            payout: Some(payout),
            claims: None,
            solo: true,
            insecure_open_port: false,
            ca_cert: Some(CERT.to_vec()),
        })
        .unwrap();
        let started = std::time::Instant::now();

        while worker.status().blocks == 0 {
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "no block over wss://: {:?}", worker.status());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let status = worker.status();
        assert!(status.connected && status.trouble.is_none(), "{status:?}");
        drop(worker);

        // The node imports it a moment after the port takes it, paying what the worker said.
        let tip: serde_json::Value = loop {
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "the block never landed");

            match ureq::get(format!("http://{api}/blocks/1")).call() {
                Ok(mut answer) => break serde_json::from_str(&answer.body_mut().read_to_string().unwrap()).unwrap(),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        };

        assert_eq!(tip["producer"].as_str().or(tip["header"]["producer"].as_str()), Some(payout.to_string().as_str()), "{tip}");
        running.stop();
    }

    #[test]
    fn a_browser_mines_alone_over_the_apis_websocket_with_the_ports_own_handshake() {
        use crate::consensus::{PowConfig, meets, work_hash};
        use crate::mining::pool::{MiningPort, OPEN_TOKEN};
        use crate::mining::protocol::{self, Hello};
        use crate::node::{Mining, Producing};
        use crate::structs::hash::{Hash, HashType};
        use tungstenite::Message;

        let listen = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let payout = Address::Ed25519([4; 32]);
        let pow = |genesis: Genesis| Genesis {
            consensus: ConsensusConfig::ProofOfWork(PowConfig { hash: HashType::default_chain(), target_block_ms: 1_000, initial_difficulty: 64, min_difficulty: 64, tuning: None, window: None }),
            ..genesis
        };
        let mining = move |node: Node| {
            node.producing(Producing {
                mine: Some(Mining {
                    reward_to: Address::Ed25519([9; 32]),
                    threads: 0,
                    gpu: false,
                    gpus: None,
                    port: Some(MiningPort { listen, token: OPEN_TOKEN.to_vec(), protocol: "chainv4".to_string(), #[cfg(feature = "pool")] pool: None, #[cfg(feature = "updates")] releases: None, solo: true, limits: crate::mining::pool::PortLimits::DEFAULT }),
                }),
                sign: None,
            })
        };
        let options = crate::api::ApiOptions {
            mining_public: Some("miner.example:4002".to_string()),
            mining_onion: Some(crate::api::MiningOnion::At("miner.onion:4100".to_string())),
            ..Default::default()
        };
        let running = Running::launch_genesis(false, false, pow, mining, options);
        let about: serde_json::Value = serde_json::from_str(&ureq::get(format!("http://{}/mining", running.api)).call().unwrap().body_mut().read_to_string().unwrap()).unwrap();

        assert_eq!((about["port"].as_u64(), about["public"].as_str(), about["ws"].as_str()), (Some(u64::from(listen.port())), Some("miner.example:4002"), Some("/mining/ws")));
        assert_eq!(about["onion"].as_str(), Some("miner.onion:4100"), "and through Tor");
        assert_eq!((about["solo"].as_bool(), about["open"].as_bool(), about["pool"].as_bool()), (Some(true), Some(true), Some(false)));

        let (mut socket, _) = tungstenite::connect(format!("ws://{}/mining/ws", running.api)).unwrap();
        let read = |socket: &mut tungstenite::WebSocket<_>| loop {
            if let Message::Text(text) = socket.read().unwrap() {
                break text.to_string();
            }
        };
        let greeting: serde_json::Value = serde_json::from_str(&read(&mut socket)).unwrap();
        let me = Hello { name: "browser".to_string(), payout: Some(payout), solo: true };
        let (hello, joining) = protocol::hello(&greeting, OPEN_TOKEN, "chainv4", &me).unwrap();
        socket.send(Message::Text(hello.to_string().into())).unwrap();
        let answer: serde_json::Value = serde_json::from_str(&read(&mut socket)).unwrap();
        let (mut sealer, mut opener) = joining.finish(OPEN_TOKEN, &answer).unwrap();
        let started = std::time::Instant::now();
        let mut said_solo = false;

        // Each work message is a block paying the payout: the first nonce found is sent.
        let block = loop {
            assert!(started.elapsed() < std::time::Duration::from_secs(30), "no block over the WebSocket");
            let message = opener.open(&read(&mut socket)).unwrap();

            match message["type"].as_str() {
                Some("solo") => said_solo = true,
                Some("work") => {
                    let id = Hash::from_bytes(HashType::default_chain(), &crate::structs::hex::from_hex(message["id"].as_str().unwrap()).unwrap()).unwrap();
                    let difficulty: u128 = message["difficulty"].as_str().unwrap().parse().unwrap();
                    let start = message["start"].as_u64().unwrap();
                    let nonce = (start..).find(|nonce| meets(work_hash(HashType::default_chain(), &id, *nonce), difficulty)).unwrap();
                    let found = serde_json::json!({ "type": "found", "job": message["job"], "nonce": nonce });
                    socket.send(Message::Text(sealer.seal(&found).into())).unwrap();
                }
                Some("verdict") if message["took"] == true => break message,
                _ => {}
            }
        };

        assert!(said_solo, "the port said it mines alone first");
        assert_eq!(block["kind"], "block");

        // The node imports it a moment after the port takes it.
        let tip: serde_json::Value = loop {
            assert!(started.elapsed() < std::time::Duration::from_secs(30), "the block never landed");

            match ureq::get(format!("http://{}/blocks/1", running.api)).call() {
                Ok(mut answer) => break serde_json::from_str(&answer.body_mut().read_to_string().unwrap()).unwrap(),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        };

        assert_eq!(tip["producer"].as_str().or(tip["header"]["producer"].as_str()), Some(payout.to_string().as_str()), "{tip}");
        running.stop();
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

    #[cfg(all(feature = "swap", feature = "mnemonic"))]
    #[test]
    fn a_multisig_swaps_with_its_quorum_in_the_htlc_and_its_members_sign_the_claim() {
        use crate::cli::multisig::{bitcoin_wallet, sign_file};
        use crate::cli::swap::{BitcoinSpend, SwapAction, SwapArgs, bitcoin, swap};
        use crate::crypto::bitcoin::multisig::{bitcoin_address, xpub_text};
        use crate::crypto::bitcoin::psbt::Psbt;
        use crate::crypto::bitcoin::tx::Network;
        use crate::crypto::mnemonic::Phrase;
        use crate::swap::htlc::Spend;
        use crate::swap::{BitcoinLeg, BitcoinParty, ChainLeg, Leg, Parties, Terms, multisig_index, secret};

        let running = Running::start(true);
        let dir = TempDir::new("multisig-swap");
        let seeds: Vec<_> = [
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "legal winner thank year wave sausage worth useful legal winner thank yellow",
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        ]
        .iter()
        .map(|words| Phrase::parse(words).unwrap().seed(""))
        .collect();
        let members = seeds.iter().enumerate().map(|(at, seed)| {
            let key = seed.key(crate::crypto::keypair::KeyType::Ed25519, 0).public();

            crate::cli::send::parse_member(&format!("{key},{},0x{}", xpub_text(&seed.multisig_account(1), Network::Test), format!("{:02x}", 0x41 + at).repeat(20))).unwrap()
        }).collect();
        send(running.args(TxArgs::CreateMultisig { members, threshold: 2 })).unwrap();
        let account = crate::structs::address::multisig_address(&DevAuthority::dev_keypair().public().address(), 0, crate::structs::hash::HashType::default_chain());
        let node = format!("http://{}", running.api);
        let wallet = bitcoin_wallet(&crate::cli::send::agent(&Default::default()).unwrap(), &node, &account).unwrap();

        // The multisig offers its coin for Bob's bitcoin: its quorum takes the HTLC's claim.
        let (secret, hash) = secret().unwrap();
        let quorum = BitcoinParty::of_multisig(&wallet, multisig_index(&hash)).unwrap();
        let multisig = Parties { chain: Some(account), bitcoin: Some(quorum.clone()), evm: None };
        let bob = Parties { chain: Some(Address::Ed25519([0xb; 32])), bitcoin: Some(BitcoinParty::Key(crate::crypto::secp256k1::public_key(&[0x22; 32]).unwrap())), evm: None };
        let pays = Leg::Chain(ChainLeg { item: None, contract: Address::Contract([0x5a; 32]), token: None, amount: u256::new(5_000), payer: None, receiver: None });
        let gets = Leg::Bitcoin(BitcoinLeg { testnet: true, sats: 100_000, payer: None, receiver: None });
        let now = crate::node::now_ms();
        let terms = Terms::offer(hash, pays, gets, now + crate::swap::FIRST_DEADLINE_MS, now + crate::swap::SECOND_DEADLINE_MS, &multisig).unwrap().answer(&bob).unwrap();

        assert_eq!((terms.to_bytes()[0], terms.htlc().unwrap().claim), (4, quorum));

        // Its claim is a PSBT for the members, written by the CLI from the node's multisig; two
        // sign, and bitcoin-finish makes the transaction.
        let psbt = dir.0.join("claim.psbt");
        let to = bitcoin_address(2, &wallet.xpubs, Network::Test, 0, 0).unwrap();
        let spend = BitcoinSpend {
            terms: terms.to_text(),
            funding: format!("{}:0", "ab".repeat(32)),
            sats: 100_000,
            key: None,
            to,
            fee_rate: Some(2),
            bitcoin: Default::default(),
            proxy: Default::default(),
            multisig: Some(account),
            multisig_index: None,
            psbt: Some(psbt.clone()),
            node: node.clone(),
        };
        let written = bitcoin(&spend, Spend::Claim { secret });
        running.stop();

        assert!(written.unwrap().contains("PSBT"));
        sign_file(&psbt, &seeds[0]).unwrap();
        sign_file(&psbt, &seeds[1]).unwrap();
        swap(SwapArgs { action: SwapAction::BitcoinFinish { terms: terms.to_text(), psbt: psbt.clone(), sats: 100_000, secret: Some(crate::structs::hex::to_hex(&secret)) } }).unwrap();

        let finished = Psbt::from_base64(&fs::read_to_string(&psbt).unwrap()).unwrap().finish_htlc(&terms.htlc().unwrap(), 100_000, Spend::Claim { secret }).unwrap();
        assert_eq!(finished.inputs[0].witness.len(), 6);
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
        let alice = Parties { chain: Some(Address::Ed25519([0xa; 32])), bitcoin: Some(crate::swap::BitcoinParty::Key(bitcoin_key(&alice_bitcoin))), evm: None };
        let bob = Parties { chain: Some(deployer), bitcoin: Some(crate::swap::BitcoinParty::Key(bitcoin_key(&bob_bitcoin))), evm: None };
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
        let action = || LegAction { terms: terms.to_text(), leg: None, node: node.clone(), eth_rpc: Vec::new(), eth_auth: Default::default(), proxy: Default::default(), tls: Default::default(), key: None, anyway: false, multisig: None, out: None };

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
        let spend = BitcoinSpend { terms: terms.to_text(), funding: format!("{}:1", "ab".repeat(32)), sats: 100_000, key: Some(key), to, fee_rate: Some(2), bitcoin: Default::default(), proxy: Default::default(), multisig: None, multisig_index: None, psbt: None, node: String::new() };
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
        assert_eq!(held(&me) - before, crate::dex::amount_out(u256::new(10_000), u256::new(1_000_000), u256::new(4_000_000), crate::dex::DEFAULT_FEE_BPS).unwrap().as_u128());
        run(DexAction::Swap { from: token_text.clone(), to: coin.clone(), amount: u256::new(20_000), slippage: 1.0 }, false, Some(contract), None).unwrap();
        let pool = read(&contract, "pool", serde_json::json!([{ "type": "address", "value": crate::dex::COIN.to_string() }, { "type": "address", "value": token_text }]));
        let share: Address = pool["values"][2]["value"].as_str().unwrap().parse().unwrap();
        let shares = read(&share, "balance_of", serde_json::json!([{ "type": "address", "value": me.to_string() }]))["values"][0]["value"].as_str().unwrap().parse::<u128>().unwrap();
        run(DexAction::Remove { a: coin.clone(), b: token_text.clone(), shares: u256::new(shares / 2) }, false, Some(contract), None).unwrap();
        let left = read(&share, "balance_of", serde_json::json!([{ "type": "address", "value": me.to_string() }]))["values"][0]["value"].as_str().unwrap().parse::<u128>().unwrap();
        assert_eq!(left, shares - shares / 2);
        let none = run(DexAction::Swap { from: coin.clone(), to: Address::Contract([9; 32]).to_string(), amount: u256::new(1), slippage: 1.0 }, false, Some(contract), None);
        assert!(none.unwrap_err().to_string().contains("no pool"));

        // Amounts far off the pool's ratio, either way: the pair at its ratio within them goes in,
        // with no slippage allowed at all. The token's allowance is spent, so it is approved first.
        let reserves = || -> (u256, u256) {
            let pool = read(&contract, "pool", serde_json::json!([{ "type": "address", "value": crate::dex::COIN.to_string() }, { "type": "address", "value": token.to_string() }]));
            let number = |at: usize| u256::from_str_radix(pool["values"][at]["value"].as_str().unwrap(), 10).unwrap();

            (number(0), number(1))
        };
        for (coin_most, token_most) in [(100_000u128, 5_000_000u128), (5_000_000, 100_000)] {
            let (coin_held, token_held) = reserves();
            let (coin_in, token_in) = crate::dex::add_amounts(coin_held, token_held, u256::new(coin_most), u256::new(token_most)).unwrap();
            let (before, sent) = (held(&me), nonce(&node));
            let approved_already = read(&token, "allowance", serde_json::json!([{ "type": "address", "value": me.to_string() }, { "type": "address", "value": contract.to_string() }]))["values"][0]["value"].as_str().unwrap().parse::<u128>().unwrap() >= token_most;
            run(DexAction::Add { a: coin.clone(), b: token_text.clone(), amount_a: u256::new(coin_most), amount_b: u256::new(token_most), slippage: 0.0 }, false, Some(contract), None).unwrap();
            assert_eq!(reserves(), (coin_held + coin_in, token_held + token_in), "at most {coin_most} and {token_most}");
            assert_eq!(u256::new(before - held(&me)), token_in);
            assert_eq!(nonce(&node) - sent, if approved_already { 1 } else { 2 });
        }

        // Approved for plenty, an add sends the add alone.
        send(running.args(TxArgs::Call { to: token, function: "approve".into(), amount: u256::ZERO, args: vec![crate::structs::tx::TxCallArgumentType::Address(contract), crate::structs::tx::TxCallArgumentType::U256(u256::new(10u128.pow(12)))] })).unwrap();
        let sent = nonce(&node);
        run(DexAction::Add { a: token_text.clone(), b: coin.clone(), amount_a: u256::new(70_000), amount_b: u256::new(9_000_000), slippage: 0.5 }, false, Some(contract), None).unwrap();
        assert_eq!(nonce(&node) - sent, 1, "no approval: the DEX may move enough already");
        let plan = crate::cli::dex::plan_add(Some((u256::new(4), u256::new(1))), u256::new(400), u256::new(400), 50).unwrap();
        assert_eq!((plan.amount_a, plan.amount_b, plan.least_a, plan.least_b, plan.opens), (u256::new(400), u256::new(100), u256::new(398), u256::new(100), false), "the least is the pair less the slippage");
        assert!(plan.said(&token, &crate::dex::COIN, u256::new(400), u256::new(400), 0.5).contains("100 of the 400 given"));
        let fresh = crate::cli::dex::plan_add(None, u256::new(5_000), u256::new(7_000), 50).unwrap();
        assert_eq!((fresh.amount_a, fresh.amount_b, fresh.least_a, fresh.least_b, fresh.opens), (u256::new(5_000), u256::new(7_000), u256::new(5_000), u256::new(7_000), true), "a new pool takes them as given");

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

        // Off the pair's ratio with no slippage allowed, both ways: the router takes the pair
        // worked out. The first approves, its allowance spent; the second has plenty left over.
        let sent = nonce(&node);
        run(DexAction::Add { a: erc20.clone(), b: coin.clone(), amount_a: u256::new(3_000_000), amount_b: u256::new(10_000), slippage: 0.0 }, true, None, Some(router.clone())).unwrap();
        assert_eq!(nonce(&node) - sent, 2, "approved, then added");
        let sent = nonce(&node);
        run(DexAction::Add { a: coin.clone(), b: erc20.clone(), amount_a: u256::new(5_000_000), amount_b: u256::new(20_000), slippage: 0.0 }, true, None, Some(router.clone())).unwrap();
        assert_eq!(nonce(&node) - sent, 1, "no approval: the router may move enough already");
        let refused =run(DexAction::Swap { from: token.to_string(), to: coin, amount: u256::new(1), slippage: 1.0 }, true, None, Some(router));
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
    /// served at its paths, and what is posted to /tx, kept and known from then on as in its mempool,
    /// unless it is refusing, as Bitcoin's nodes refuse a replacement they will not take.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    struct FakeEsplora {
        url: String,
        routes: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
        posted: Arc<std::sync::Mutex<Vec<String>>>,
        refusing: Arc<AtomicBool>,
    }

    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    impl FakeEsplora {
        fn start() -> FakeEsplora {
            use std::io::{BufRead, BufReader, Read, Write};

            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let routes: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> = Arc::default();
            let posted: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
            let refusing: Arc<AtomicBool> = Arc::default();
            let (served, kept, refuses) = (routes.clone(), posted.clone(), refusing.clone());

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
                        "POST" if refuses.load(Ordering::Relaxed) => Err("insufficient fee, rejecting replacement".to_string()),
                        "POST" => {
                            let hex = String::from_utf8(body).unwrap();
                            let (tx, _) = crate::crypto::bitcoin::tx::Transaction::read(&crate::structs::hex::from_hex(&format!("0x{hex}")).unwrap()).unwrap();
                            let txid = FakeBitcoin::display(&crate::crypto::bitcoin::sha256d(&tx.base()));
                            let mut routes = served.lock().unwrap();
                            routes.insert(format!("/tx/{txid}/hex"), hex.clone());
                            routes.entry(format!("/tx/{txid}/status")).or_insert_with(|| serde_json::json!({ "confirmed": false }).to_string());
                            kept.lock().unwrap().push(hex);
                            Ok(Some(txid))
                        }
                        _ => Ok(served.lock().unwrap().get(&path).cloned()),
                    };
                    let (status, body) = match answer {
                        Ok(Some(body)) => ("200 OK", body),
                        Ok(None) => ("404 Not Found", "not found".to_string()),
                        Err(why) => ("400 Bad Request", why),
                    };
                    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }
            });

            FakeEsplora { url, routes, posted, refusing }
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

            let first_tx = crate::vm::tests::btc_relay_coinbase(self.time);
            let coinbase = sha256d(&first_tx);
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
            esplora.serve(&format!("/block/{}/txid/0", FakeBitcoin::display(&hash)), FakeBitcoin::display(&coinbase));
            esplora.serve("/blocks/tip/height", height.to_string());
            let serve_tx = |tx: &[u8], id: &[u8; 32], merkle: Vec<String>, pos: u64| {
                let txid = FakeBitcoin::display(id);
                esplora.serve(&format!("/tx/{txid}/hex"), crate::structs::hex::to_hex(tx).trim_start_matches("0x").to_string());
                esplora.serve(&format!("/tx/{txid}/merkle-proof"), serde_json::json!({ "block_height": height, "merkle": merkle, "pos": pos }).to_string());
                esplora.serve(&format!("/tx/{txid}/status"), serde_json::json!({ "confirmed": true, "block_height": height }).to_string());
            };
            serve_tx(&first_tx, &coinbase, ids[1..].iter().map(|id| FakeBitcoin::display(id)).collect(), 0);

            for (tx, id) in txs.iter().zip(&ids[1..]) {
                serve_tx(tx, id, vec![FakeBitcoin::display(&coinbase)], 1);
            }

            (self.tip, self.height, self.time) = (hash, height, self.time + 600);
        }
    }

    /// Bitcoin reorganizes under the relay: the headers from just past its tip build on nothing it
    /// holds, so a sync goes back to where the two last agreed.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn a_relay_left_on_a_branch_bitcoin_dropped_syncs_onto_the_one_it_took() {
        use crate::cli::bridge::{BridgeAction, BridgeArgs, bridge};
        use crate::cli::bridge::btc::BtcAction;

        let me = DevAuthority::dev_keypair().public().address();
        let running = Running::launch_genesis(true, false, |genesis| genesis, |node| node, Default::default());
        let node = format!("http://{}", running.api);
        let mut bitcoin = FakeBitcoin::start();
        let dir = TempDir::new("btc-reorg");
        let wasm = dir.0.join("btc_relay.wasm");
        fs::write(&wasm, crate::vm::tests::example("btc_relay")).unwrap();
        let relay = deployed_at(&me, 0, crate::structs::hash::HashType::default_chain());
        send(running.args(TxArgs::Deploy { lua: None, wasm: Some(wasm), evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();
        let esplora = bitcoin.esplora.url.clone();
        let run = |action: BtcAction| {
            bridge(BridgeArgs {
                action: BridgeAction::Btc { action },
                node: node.clone(),
                tls: Default::default(),
                key: None,
                gateway: None,
                eth_rpc: Vec::new(),
                evm_key: None,
                btc_gateway: None,
                bitcoin: crate::cli::outside::BitcoinArgs { esplora: vec![esplora.clone()], ..Default::default() },
                beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(),
                evm_gateways: Vec::new(),
                max_fee_rate: None,
                cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS,
                bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS,
            })
        };
        let tip = || -> (String, u64) {
            let body = serde_json::json!({ "contract": relay.to_string(), "function": "tip", "args": [] });
            let answer: serde_json::Value = serde_json::from_str(&ureq::post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap();
            let hash = crate::structs::hex::from_hex(answer["values"][0]["value"].as_str().unwrap()).unwrap();

            (FakeBitcoin::display(&hash), answer["values"][1]["value"].as_str().unwrap().parse().unwrap())
        };

        run(BtcAction::RelayStart { relay, height: 0, network: Some("regtest".into()) }).unwrap();
        bitcoin.block(&[], false);
        let (shared, shared_height) = (bitcoin.tip, bitcoin.height);
        bitcoin.block(&[], false);
        bitcoin.block(&[], false);
        run(BtcAction::RelaySync { relay, batch: 10 }).unwrap();
        assert_eq!(tip(), (FakeBitcoin::display(&bitcoin.tip), 3));

        // Bitcoin leaves the last two behind, for a longer branch from the block before.
        (bitcoin.tip, bitcoin.height) = (shared, shared_height);

        for _ in 0..3 {
            bitcoin.block(&[], false);
        }

        run(BtcAction::RelaySync { relay, batch: 10 }).unwrap();
        assert_eq!(tip(), (FakeBitcoin::display(&bitcoin.tip), 4), "on the branch Bitcoin took");

        // In step again, it only takes what is new.
        bitcoin.block(&[], false);
        run(BtcAction::RelaySync { relay, batch: 10 }).unwrap();
        assert_eq!(tip(), (FakeBitcoin::display(&bitcoin.tip), 5));
        running.stop();
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
            btc: None,
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
                bitcoin: crate::cli::outside::BitcoinArgs { esplora: vec![esplora.clone()], ..Default::default() },
                beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(),
                evm_gateways: Vec::new(),
                max_fee_rate: None,
                cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS,
                bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS,
            })
        };
        run(BtcAction::RelayStart { relay, height: 0, network: Some("regtest".into()) }, None, None).unwrap();

        // The gateway, after its wBTC template, on that relay.
        run(BtcAction::Deploy { relay, fee_cap: 10_000, fee_floor: 2, grace_hours: 1, least_deposit: 10_000 }, None, None).unwrap();
        let gateway = deployed_at(&me, start + 2, hash);
        assert_eq!(gateway, announced);
        run(BtcAction::Address { recipient: me, network: None }, None, None).unwrap();

        // 50,000 satoshis to the dev key's deposit address, buried two deep, shown, minted.
        let output = read(&gateway, "deposit_script", serde_json::json!([{ "type": "address", "value": me.to_string() }]))[1]["value"].as_str().map(|hex| crate::structs::hex::from_hex(hex).unwrap()).unwrap();

        // The deposit address is written for the Bitcoin the relay follows, regtest here, with or
        // without Esplora, and whatever --network would say: main is refused, not written.
        let written = |network: Option<&str>, esplora: Option<String>| {
            crate::cli::bridge::Bridge::new(BridgeArgs {
                action: BridgeAction::Btc { action: BtcAction::Pass },
                node: node.clone(),
                tls: Default::default(),
                key: None,
                gateway: None,
                eth_rpc: Vec::new(),
                evm_key: None,
                btc_gateway: None,
                bitcoin: crate::cli::outside::BitcoinArgs { esplora: esplora.into_iter().collect(), ..Default::default() },
                beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(),
                evm_gateways: Vec::new(),
                max_fee_rate: None,
                cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS,
                bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS,
            })
            .unwrap()
            .deposit_address(&me, network)
        };
        let regtest = crate::cli::bridge::btc::Followed::Regtest;
        let (address, network, why) = written(None, None).unwrap();
        assert_eq!((address.as_str(), network), (crate::structs::bech32::encode_segwit("bcrt", &output[2..]).as_str(), regtest), "{why}");
        assert!(address.starts_with("bcrt1q"), "{address}");
        assert_eq!(written(Some("regtest"), Some(esplora.clone())).unwrap().0, address);
        assert!(written(Some("main"), None).unwrap_err().to_string().contains("not the Bitcoin here"));

        // What the wallet works out alone is what a real chain's gateway says, under the id it says.
        let chain_id = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node}/status")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["chain_id"].as_u64().unwrap();
        assert_eq!(output, crate::crypto::bitcoin::tx::p2wsh(&crate::bridge::btc_deposit_script(&me, chain_id, &gateway, federation.threshold, &federation.signers)));

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

        // An address on another Bitcoin than the relay's is refused, and nothing is burned: the
        // relay follows regtest, so bc1q… and tb1q… would pay a program on a network no one meant.
        for (hrp, network) in [("bc", "main"), ("tb", "test")] {
            let wrong = crate::structs::bech32::encode_segwit(hrp, &[0xb0; 20]);
            let why = run(BtcAction::Withdraw { address: wrong, sats: 20_000, most_fee: None, network: None }, None, None).unwrap_err().to_string();
            assert!(why.contains(&format!("Bitcoin's {network} network")) && why.contains("bcrt1…"), "{why}");
        }
        assert!(run(BtcAction::Withdraw { address: crate::structs::bech32::encode_segwit("bc", &[0xb0; 20]), sats: 20_000, most_fee: None, network: Some("main".into()) }, None, None).unwrap_err().to_string().contains("is not the Bitcoin here"));
        assert_eq!(wbtc_of(&me), "50000");

        // 20,000 withdrawn to a bcrt1q… address, proposed with a 1,000 fee, signed by two signers.
        let to = crate::structs::bech32::encode_segwit("bcrt", &[0xb0; 20]);
        run(BtcAction::Withdraw { address: to, sats: 20_000, most_fee: None, network: None }, None, None).unwrap();
        assert_eq!(wbtc_of(&me), "30000");
        let signer = |secret: u8| {
            let key = dir.0.join(format!("signer-{secret}.key"));
            fs::write(&key, Keypair::from_private(PrivateKey::Secp256k1([secret; 32])).to_key_file()).unwrap();
            Some(key)
        };
        // Not by a stranger, nor by a stranger's key: only a signer of the federation proposes.
        assert!(run(BtcAction::Propose { fee: Some(1_000) }, None, None).unwrap_err().to_string().contains("only a signer"));
        assert!(run(BtcAction::Propose { fee: Some(1_000) }, None, signer(99)).unwrap_err().to_string().contains("not a signer"));
        run(BtcAction::Propose { fee: Some(1_000) }, None, signer(21)).unwrap();

        for secret in [21u8, 22] {
            let key = signer(secret).unwrap();
            run(BtcAction::Sign, None, Some(key.clone())).unwrap();
            run(BtcAction::Sign, None, Some(key)).unwrap();
        }

        // Assembled and broadcast: two signatures and the script in its witness.
        run(BtcAction::Finish { proposal: 1 }, None, None).unwrap();
        let posted = bitcoin.esplora.posted.lock().unwrap().pop().unwrap();
        let (signed, witnessed) = Transaction::read(&crate::structs::hex::from_hex(&format!("0x{posted}")).unwrap()).unwrap();

        assert!(witnessed);
        assert_eq!(signed.inputs[0].witness.len(), 4, "the extra pop, two signatures, the script");
        assert_eq!((signed.outputs[0].value, signed.outputs[1].value), (19_000, 30_000), "the fee off the withdrawal, none off the backing");

        // Mined and buried: shown released, the withdrawal paid, the change held.
        let release = signed.base();
        bitcoin.block(std::slice::from_ref(&release), false);
        bitcoin.block(&[], false);
        run(BtcAction::RelaySync { relay, batch: 10 }, None, None).unwrap();
        run(BtcAction::Released { proposal: 1 }, None, None).unwrap();

        assert_eq!(read(&gateway, "withdrawal", serde_json::json!([{ "type": "u64", "value": "1" }]))[2]["value"], "paid");
        assert_eq!(read(&gateway, "utxo_count", serde_json::json!([]))[0]["value"], "2");
        assert!(run(BtcAction::Propose { fee: Some(1_000) }, None, signer(21)).unwrap_err().to_string().contains("no withdrawals are queued"));

        // Another withdrawal, proposed from the change, signed by one, then cancelled by two: a
        // signer that voted does not sign it, and the next one spends the same input.
        run(BtcAction::Withdraw { address: crate::structs::bech32::encode_segwit("bcrt", &[0xb0; 20]), sats: 10_000, most_fee: Some(500), network: None }, None, None).unwrap();

        // A fee past what it agreed to lose takes nothing, until it says otherwise.
        assert!(run(BtcAction::Propose { fee: Some(1_000) }, None, signer(21)).unwrap_err().to_string().contains("less than its share"));
        run(BtcAction::SetMostFee { withdrawal: 2, most_fee: 1_000 }, None, None).unwrap();
        run(BtcAction::Propose { fee: Some(1_000) }, None, signer(21)).unwrap();
        let state = |proposal: &str| read(&gateway, "proposal", serde_json::json!([{ "type": "u64", "value": proposal }]))[4]["value"].as_str().unwrap().to_string();
        let posted = |proposal: &str| read(&gateway, "input_signatures", serde_json::json!([{ "type": "u64", "value": proposal }, { "type": "u64", "value": "0" }])).as_array().unwrap().len();
        let signer_at = |secret: u8| dir.0.join(format!("signer-{secret}.key"));
        fs::write(signer_at(23), Keypair::from_private(PrivateKey::Secp256k1([23; 32])).to_key_file()).unwrap();
        run(BtcAction::Cancel { proposal: 2 }, None, Some(signer_at(21))).unwrap();
        run(BtcAction::Sign, None, Some(signer_at(21))).unwrap();
        assert_eq!(posted("2"), 2, "the script and threshold, no signature: it voted to cancel");
        run(BtcAction::Sign, None, Some(signer_at(22))).unwrap();
        assert_eq!(posted("2"), 4, "one of the two it takes");
        assert!(run(BtcAction::Finish { proposal: 2 }, None, None).unwrap_err().to_string().contains("of the 2 signatures"));
        run(BtcAction::Cancel { proposal: 2 }, None, Some(signer_at(22))).unwrap();
        assert_eq!(state("2"), "cancelled");
        assert_eq!(read(&gateway, "withdrawal", serde_json::json!([{ "type": "u64", "value": "2" }]))[2]["value"], "queued");
        assert!(run(BtcAction::Finish { proposal: 2 }, None, None).unwrap_err().to_string().contains("is cancelled"));
        // Too little for its size, the tools say so before the gateway has to.
        assert!(run(BtcAction::Propose { fee: Some(1) }, None, signer(21)).unwrap_err().to_string().contains("under the floor"));
        run(BtcAction::Propose { fee: Some(1_000) }, None, signer(21)).unwrap();
        assert_eq!(state("3"), "open");

        // The cancelled one was finished anyway, with a signature that came too late for the board:
        // Bitcoin mined it, and the gateway is shown. What was proposed since conflicts with it, so
        // its withdrawal is paid once, and a signer going over the new one finds that from the
        // gateway's state, though the gateway would still take its signature.
        let cancelled = read(&gateway, "proposal", serde_json::json!([{ "type": "u64", "value": "2" }]))[0]["value"].as_str().map(|hex| crate::structs::hex::from_hex(hex).unwrap()).unwrap();
        bitcoin.block(std::slice::from_ref(&cancelled), false);
        bitcoin.block(&[], false);
        run(BtcAction::RelaySync { relay, batch: 10 }, None, None).unwrap();
        run(BtcAction::Released { proposal: 2 }, None, None).unwrap();
        assert_eq!(state("2"), "released");
        assert_eq!(read(&gateway, "withdrawal", serde_json::json!([{ "type": "u64", "value": "2" }]))[2]["value"], "paid");
        run(BtcAction::Sign, None, Some(signer_at(23))).unwrap();
        assert_eq!(posted("3"), 2, "not signed: its withdrawal is paid");
        run(BtcAction::Cancel { proposal: 3 }, None, Some(signer_at(21))).unwrap();
        run(BtcAction::Cancel { proposal: 3 }, None, Some(signer_at(22))).unwrap();
        assert_eq!(state("3"), "cancelled");
        running.stop();
    }

    /// A chain with the federation 21, 22 and 23, two to sign, a Bitcoin gateway on a synced relay
    /// holding three deposits, and a Bitcoin whose network asks what a test says: what the signers'
    /// own passes over stuck releases are tried on.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    struct StuckReleases {
        running: Running,
        node: String,
        bitcoin: FakeBitcoin,
        esplora: String,
        relay: Address,
        gateway: Address,
        dir: TempDir,
    }

    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    impl StuckReleases {
        fn start(name: &str) -> StuckReleases {
            use crate::chain::{IndexKind, KeepFrom};
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
                btc: None,
            };
            let me = DevAuthority::dev_keypair().public().address();
            let hash = crate::structs::hash::HashType::default_chain();
            let running = Running::launch_genesis(true, false, |genesis| Genesis { federation: Some(federation.clone()), ..genesis }, |node: Node| node.keep_index(IndexKind::Events, Some(KeepFrom::Height(0))).unwrap(), Default::default());
            let node = format!("http://{}", running.api);
            let bitcoin = FakeBitcoin::start();
            let dir = TempDir::new(name);
            let esplora = bitcoin.esplora.url.clone();
            let (relay, gateway) = (deployed_at(&me, 3, hash), deployed_at(&me, 6, hash));
            let mut fixture = StuckReleases { running, node, bitcoin, esplora, relay, gateway, dir };

            for secret in [21u8, 22, 23] {
                let payer = Keypair::from_private(PrivateKey::Secp256k1([secret; 32])).public().address();
                send(fixture.running.args(TxArgs::Transfer { to: payer.into(), amount: u256::from(10u64).pow(20) })).unwrap();
            }

            let wasm = fixture.dir.0.join("btc_relay.wasm");
            fs::write(&wasm, crate::vm::tests::example("btc_relay")).unwrap();
            send(fixture.running.args(TxArgs::Deploy { lua: None, wasm: Some(wasm), evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();
            fixture.run(BtcAction::RelayStart { relay, height: 0, network: Some("regtest".into()) }).unwrap();
            fixture.run(BtcAction::Deploy { relay, fee_cap: 100_000, fee_floor: 2, grace_hours: 1, least_deposit: 10_000 }).unwrap();
            let output = fixture.read("deposit_script", serde_json::json!([{ "type": "address", "value": me.to_string() }]))[1]["value"].as_str().map(|hex| crate::structs::hex::from_hex(hex).unwrap()).unwrap();

            for sats in [300_000, 300_001, 300_002] {
                let deposit = Transaction { version: 2, inputs: vec![TxIn { txid: [0x5e; 32], vout: 0, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }], outputs: vec![TxOut { value: sats, script: output.clone() }], lock_time: 0 }.base();
                fixture.bitcoin.block(std::slice::from_ref(&deposit), false);
                fixture.bitcoin.block(&[], false);
                fixture.run(BtcAction::RelaySync { relay, batch: 10 }).unwrap();
                fixture.run(BtcAction::Deposit { txid: FakeBitcoin::display(&crate::crypto::bitcoin::sha256d(&deposit)), vout: 0, recipient: me }).unwrap();
            }

            fixture.asks(5);

            fixture
        }

        /// The network asks this much a virtual byte, however soon.
        fn asks(&self, rate: u64) {
            self.bitcoin.esplora.serve("/fee-estimates", format!(r#"{{"1":{rate}.0,"3":{rate}.0}}"#));
        }

        fn args(&self, action: crate::cli::bridge::btc::BtcAction, secret: Option<u8>) -> crate::cli::bridge::BridgeArgs {
            use crate::crypto::keypair::{Keypair, PrivateKey};

            let key = secret.map(|secret| {
                let key = self.dir.0.join(format!("signer-{secret}.key"));
                fs::write(&key, Keypair::from_private(PrivateKey::Secp256k1([secret; 32])).to_key_file()).unwrap();

                key
            });

            crate::cli::bridge::BridgeArgs {
                action: crate::cli::bridge::BridgeAction::Btc { action },
                node: self.node.clone(),
                tls: Default::default(),
                key: key.clone(),
                gateway: None,
                eth_rpc: Vec::new(),
                evm_key: key,
                btc_gateway: Some(self.gateway),
                bitcoin: crate::cli::outside::BitcoinArgs { esplora: vec![self.esplora.clone()], ..Default::default() },
                beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(),
                evm_gateways: Vec::new(),
                max_fee_rate: None,
                cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS,
                bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS,
            }
        }

        /// By the dev key, with no signer's.
        fn run(&self, action: crate::cli::bridge::btc::BtcAction) -> crate::cli::CliResult<()> {
            crate::cli::bridge::bridge(self.args(action, None))
        }

        /// One signer's pass, as it is left running.
        fn pass(&self, secret: u8) {
            crate::cli::bridge::bridge(self.args(crate::cli::bridge::btc::BtcAction::Pass, Some(secret))).unwrap();
        }

        /// One signer's pass, past an operator's own limit on what it signs for.
        fn pass_limited(&self, secret: u8, most: u64) {
            crate::cli::bridge::bridge(crate::cli::bridge::BridgeArgs { max_fee_rate: Some(most), ..self.args(crate::cli::bridge::btc::BtcAction::Pass, Some(secret)) }).unwrap();
        }

        /// One signer's pass, with an age limit of this many milliseconds of block time.
        fn pass_past(&self, secret: u8, cancel_after: u64) {
            crate::cli::bridge::bridge(crate::cli::bridge::BridgeArgs { cancel_after, ..self.args(crate::cli::bridge::btc::BtcAction::Pass, Some(secret)) }).unwrap();
        }

        fn propose_by_hand(&self, secret: u8, fee: u64) {
            crate::cli::bridge::bridge(self.args(crate::cli::bridge::btc::BtcAction::Propose { fee: Some(fee) }, Some(secret))).unwrap();
        }

        fn withdraw(&self) -> String {
            let to = crate::structs::bech32::encode_segwit("bcrt", &[0xb0; 20]);
            self.run(crate::cli::bridge::btc::BtcAction::Withdraw { address: to, sats: 200_000, most_fee: Some(100_000), network: None }).unwrap();

            self.count("withdrawal_count")
        }

        fn read(&self, function: &str, args: serde_json::Value) -> serde_json::Value {
            let body = serde_json::json!({ "contract": self.gateway.to_string(), "function": function, "args": args });

            serde_json::from_str::<serde_json::Value>(&ureq::post(format!("{}/call", self.node)).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap()["values"].clone()
        }

        fn count(&self, function: &str) -> String {
            self.read(function, serde_json::json!([]))[0]["value"].as_str().unwrap().to_string()
        }

        fn proposal(&self, id: &str) -> serde_json::Value {
            self.read("proposal", serde_json::json!([{ "type": "u64", "value": id }]))
        }

        fn state(&self, id: &str) -> String {
            self.proposal(id)[4]["value"].as_str().unwrap().to_string()
        }

        fn withdrawal_state(&self, id: &str) -> String {
            self.read("withdrawal", serde_json::json!([{ "type": "u64", "value": id }]))[2]["value"].as_str().unwrap().to_string()
        }

        fn anchor(&self, id: &str) -> Option<String> {
            self.read("anchor", serde_json::json!([{ "type": "u64", "value": id }]))[0]["value"].as_str().map(str::to_string)
        }

        fn voted(&self, id: &str, secret: u8) -> bool {
            let key = crate::structs::hex::to_hex(&crate::crypto::secp256k1::public_key(&[secret; 32]).unwrap());

            self.read("voted", serde_json::json!([{ "type": "u64", "value": id }, { "type": "bytes", "value": key }]))[0]["value"] == true
        }

        fn signed(&self, id: &str) -> usize {
            (self.read("input_signatures", serde_json::json!([{ "type": "u64", "value": id }, { "type": "u64", "value": "0" }])).as_array().unwrap().len() - 2) / 2
        }

        fn spends(&self, id: &str) -> Vec<String> {
            let (tx, _) = crate::crypto::bitcoin::tx::Transaction::read(&crate::structs::hex::from_hex(self.proposal(id)[0]["value"].as_str().unwrap()).unwrap()).unwrap();

            tx.inputs.iter().map(|input| format!("{}:{}", crate::structs::hex::to_hex(&input.txid).trim_start_matches("0x"), input.vout)).collect()
        }

        fn nonce(&self, secret: u8) -> u64 {
            use crate::crypto::keypair::{Keypair, PrivateKey};

            let of = Keypair::from_private(PrivateKey::Secp256k1([secret; 32])).public().address();

            serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{}/accounts/{of}", self.node)).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap()
        }

        /// One signer's pass, bumping what is stuck after this many milliseconds of block time.
        fn pass_bumping(&self, secret: u8, bump_after: u64) {
            crate::cli::bridge::bridge(crate::cli::bridge::BridgeArgs { bump_after, ..self.args(crate::cli::bridge::btc::BtcAction::Pass, Some(secret)) }).unwrap();
        }

        fn bump_by_hand(&self, secret: u8, release: u64, rate: Option<u64>, child: bool) -> crate::cli::CliResult<()> {
            crate::cli::bridge::bridge(self.args(crate::cli::bridge::btc::BtcAction::Bump { release, rate, child }, Some(secret)))
        }

        fn kind(&self, id: &str) -> String {
            self.proposal(id)[5]["value"].as_str().unwrap().to_string()
        }

        /// Bitcoin mines these proposals, a block each, and buries the last; the relay follows.
        fn mine(&mut self, ids: &[&str]) {
            for id in ids {
                let raw = crate::structs::hex::from_hex(self.proposal(id)[0]["value"].as_str().unwrap()).unwrap();
                self.bitcoin.block(&[raw], false);
            }

            self.bitcoin.block(&[], false);
            self.run(crate::cli::bridge::btc::BtcAction::RelaySync { relay: self.relay, batch: 10 }).unwrap();
        }

        /// Whether Bitcoin's nodes refuse what is sent them, as a replacement they will not take.
        fn refusing(&self, refusing: bool) {
            self.bitcoin.esplora.refusing.store(refusing, Ordering::Relaxed);
        }
    }

    /// Three signers' passes at once against one release Bitcoin mined: each would show it, as on the
    /// devnet, where all three did in one block and two paid for nothing. Now each waits its turn, a
    /// block after the one before, and finds it shown or on its way, so the first alone pays.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn three_signers_passes_against_one_mined_release_send_released_once() {
        let mut fixture = StuckReleases::start("btc-race");
        let id = fixture.withdraw();
        fixture.pass(21);
        fixture.pass(22);
        assert_eq!(fixture.signed("1"), 2);
        fixture.mine(&["1"]);
        let before = [21, 22, 23].map(|secret| fixture.nonce(secret));

        std::thread::scope(|scope| {
            for secret in [23u8, 22, 21] {
                let fixture = &fixture;
                scope.spawn(move || fixture.pass(secret));
            }
        });

        assert_eq!((fixture.state("1").as_str(), fixture.withdrawal_state(&id).as_str()), ("released", "paid"));
        let after = [21, 22, 23].map(|secret| fixture.nonce(secret));
        assert_eq!(after, [before[0] + 1, before[1], before[2]], "the first in turn alone sent it");
        fixture.running.stop();
    }

    /// A release with all its signatures that Bitcoin leaves unmined while the network asks far more:
    /// past the delay the first signer in turn replaces it, the others sign the replacement as stuck,
    /// and it is broadcast; still unmined past the delay again, a child pays for it; and what Bitcoin
    /// mines is shown, the rest closed, the child's fee charged to the withdrawal.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn a_stuck_release_is_replaced_by_fee_then_paid_for_by_a_child_and_what_is_mined_is_shown() {
        let mut fixture = StuckReleases::start("btc-bump");
        let id = fixture.withdraw();
        fixture.pass(21);
        fixture.pass(22);
        let first_fee = fixture.read("withdrawal", serde_json::json!([{ "type": "u64", "value": id }]))[5]["value"].as_str().unwrap().parse::<u64>().unwrap();

        // Within the delay nothing is bumped, however far under it pays.
        fixture.asks(50);
        fixture.pass(21);
        assert_eq!(fixture.count("proposal_count"), "1");

        // Past it, the first in turn replaces it; the others sign it, as their Esplora says it is stuck.
        fixture.pass_bumping(21, 0);
        assert_eq!((fixture.count("proposal_count").as_str(), fixture.kind("2").as_str()), ("2", "replacement"));
        fixture.pass_bumping(22, 0);
        fixture.pass_bumping(23, 0);
        assert_eq!(fixture.signed("2"), 2);
        fixture.pass_bumping(21, 0);
        let posted = fixture.bitcoin.esplora.posted.lock().unwrap().len();
        assert!(posted >= 2, "the release and its replacement");

        // The replacement goes unmined past the delay again, with the network asking more: a child.
        fixture.asks(100);
        fixture.pass_bumping(21, 0);
        assert_eq!((fixture.count("proposal_count").as_str(), fixture.kind("3").as_str()), ("3", "child"));
        assert_eq!(fixture.proposal("3")[6]["value"], "2", "paying for the version Bitcoin holds");
        fixture.pass_bumping(22, 0);
        fixture.pass_bumping(23, 0);
        assert_eq!(fixture.signed("3"), 2);
        fixture.pass_bumping(21, 0);
        assert!(fixture.bitcoin.esplora.posted.lock().unwrap().len() > posted, "the child broadcast");

        // Mined, the replacement and its child: both shown, the release closed, what each cost charged.
        fixture.mine(&["2", "3"]);
        fixture.pass(22);
        assert_eq!([fixture.state("1"), fixture.state("2"), fixture.state("3")], ["replaced", "released", "released"]);
        assert_eq!(fixture.withdrawal_state(&id), "paid");
        let fee_of = |id: &str| fixture.proposal(id)[7]["value"].as_str().unwrap().parse::<u64>().unwrap();
        let charged = fixture.read("withdrawal", serde_json::json!([{ "type": "u64", "value": id }]))[5]["value"].as_str().unwrap().parse::<u64>().unwrap();
        assert!(fee_of("2") > first_fee);
        assert_eq!(charged, fee_of("2") + fee_of("3"));
        assert_eq!(fixture.count("shortfall"), fee_of("3").to_string());

        // The next release a pass proposes repays some of it, as the gateway works it out, and the
        // others sign it; shown, the shortfall falls by just that.
        fixture.asks(5);
        fixture.withdraw();
        fixture.pass(21);
        fixture.pass(22);
        assert_eq!((fixture.count("proposal_count").as_str(), fixture.signed("4")), ("4", 2));
        let repaid: u64 = fixture.proposal("4")[9]["value"].as_str().unwrap().split(',').filter_map(|share| share.parse::<u64>().ok()).sum();
        assert!(repaid > 0 && repaid <= fee_of("4") / 5, "{repaid}");
        assert_eq!(fixture.read("shortfall", serde_json::json!([]))[1]["value"], repaid.to_string());
        let child_fee = fee_of("3");
        fixture.mine(&["4"]);
        fixture.pass(21);
        assert_eq!(fixture.state("4"), "released");
        assert_eq!(fixture.count("shortfall"), (child_fee - repaid).to_string());
        fixture.running.stop();
    }

    /// A replacement Bitcoin's nodes refuse, as when a recipient pinned the release with a large child
    /// of their own: a child pays for the version they hold at once, and the replacement is not sent
    /// again while it does.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn a_replacement_bitcoin_refuses_falls_back_to_a_child_at_once() {
        let mut fixture = StuckReleases::start("btc-pinned");
        fixture.withdraw();
        fixture.pass(21);
        fixture.pass(22);
        fixture.asks(50);
        fixture.pass_bumping(21, 0);
        assert_eq!(fixture.kind("2"), "replacement");

        fixture.refusing(true);
        fixture.pass_bumping(22, 0);
        fixture.pass_bumping(21, 0);
        assert_eq!(fixture.signed("2"), 2);
        assert_eq!((fixture.count("proposal_count").as_str(), fixture.kind("3").as_str()), ("3", "child"));
        assert_eq!(fixture.proposal("3")[6]["value"], "1", "paying for the release Bitcoin holds");

        fixture.refusing(false);
        let posted = fixture.bitcoin.esplora.posted.lock().unwrap().clone();
        fixture.pass_bumping(22, 0);
        fixture.pass_bumping(23, 0);
        fixture.pass_bumping(21, 0);
        let sent: Vec<String> = fixture.bitcoin.esplora.posted.lock().unwrap()[posted.len()..].to_vec();
        let txid = |hex: &str| FakeBitcoin::display(&crate::crypto::bitcoin::sha256d(&crate::crypto::bitcoin::tx::Transaction::read(&crate::structs::hex::from_hex(&format!("0x{hex}")).unwrap()).unwrap().0.base()));
        let of = |id: &str| FakeBitcoin::display(&crate::structs::hex::from_hex(fixture.proposal(id)[1]["value"].as_str().unwrap()).unwrap());
        assert_eq!(sent.iter().map(|hex| txid(hex)).collect::<Vec<_>>(), [of("3")], "the child alone");

        fixture.mine(&["1", "3"]);
        fixture.pass(21);
        assert_eq!([fixture.state("1"), fixture.state("2"), fixture.state("3")], ["released", "replaced", "released"]);
        fixture.running.stop();
    }

    /// An operator bumps by hand, sooner than the policy would; the others sign only while their
    /// Esplora says it is stuck, and leave one that is not unsigned, with no vote until the age limit.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn a_bump_by_hand_is_signed_by_the_others_only_while_the_release_is_stuck() {
        let fixture = StuckReleases::start("btc-bump-by-hand");
        fixture.withdraw();
        fixture.pass(21);
        fixture.pass(22);

        // The network asks what it pays: needless, so no one signs it, and no one votes yet.
        fixture.bump_by_hand(23, 1, Some(20), false).unwrap();
        assert_eq!(fixture.kind("2"), "replacement");
        assert!(fixture.bump_by_hand(23, 1, Some(21), false).unwrap_err().to_string().contains("waits for its signatures"), "one at a time");
        fixture.pass(21);
        fixture.pass(22);
        assert_eq!((fixture.signed("2"), fixture.voted("2", 21)), (0, false));

        // Past the age limit it is voted down alone, and the release stands.
        fixture.pass_past(21, 0);
        fixture.pass_past(22, 0);
        assert_eq!((fixture.state("1").as_str(), fixture.state("2").as_str()), ("open", "cancelled"));

        // The network asks more: by hand, within the hour, it is signed.
        fixture.asks(40);
        fixture.bump_by_hand(23, 1, None, false).unwrap();
        fixture.pass(21);
        fixture.pass(22);
        assert_eq!((fixture.kind("3").as_str(), fixture.signed("3")), ("replacement", 2));
        fixture.running.stop();
    }

    /// A release one signer proposed at a fee the others refuse: each votes it down at once, it is
    /// cancelled, and the next pass pays its withdrawal at what the network asks, through its anchor.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn a_release_refused_as_overpriced_is_voted_down_by_two_signers_and_proposed_again_through_its_anchor() {
        let fixture = StuckReleases::start("btc-voted-down");
        let id = fixture.withdraw();

        // About 100 a virtual byte, where signers take up to 25.
        fixture.propose_by_hand(23, 20_000);
        fixture.pass(21);
        assert_eq!((fixture.voted("1", 21), fixture.state("1").as_str(), fixture.signed("1")), (true, "open", 0));

        // Voting again costs nothing, as it does not.
        let paid = fixture.nonce(21);
        fixture.pass(21);
        assert_eq!(fixture.nonce(21), paid);

        // Two is the threshold: its withdrawal goes back, tied to one of what it spent.
        fixture.pass(22);
        assert_eq!(fixture.state("1"), "cancelled");
        assert_eq!(fixture.withdrawal_state(&id), "queued");
        let anchor = fixture.anchor(&id).expect("anchored as it was cancelled");
        assert!(fixture.spends("1").contains(&anchor));

        // Next pass: at what the network asks, a lower rate than was refused, spending that anchor, so it
        // and the cancelled one can never both be mined. The others sign it this time.
        fixture.pass(21);
        assert_eq!(fixture.count("proposal_count"), "2");
        assert!(fixture.spends("2").contains(&anchor), "{:?} spends {anchor}", fixture.spends("2"));
        assert_eq!(fixture.withdrawal_state(&id), "proposed");
        fixture.pass(22);
        assert_eq!((fixture.state("2").as_str(), fixture.signed("2")), ("open", 2));
        fixture.running.stop();
    }

    /// The network asks what two signers will not sign for, and the third goes on proposing it. Fixed
    /// costs each pass are fine; a proposal, refusal and cancel going round every pass are not.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn a_release_cancelled_for_its_rate_is_not_proposed_again_at_that_rate_until_the_limit() {
        let fixture = StuckReleases::start("btc-no-flapping");
        let id = fixture.withdraw();
        fixture.asks(100);

        // Who would refuse it does not propose it, so it does not vote against itself.
        fixture.pass_limited(22, 25);
        assert_eq!(fixture.count("proposal_count"), "0", "past what this signer signs for");

        // The third signer's limit goes by the network, so it proposes and signs; the others vote it down.
        fixture.pass(21);
        assert_eq!((fixture.count("proposal_count").as_str(), fixture.signed("1")), ("1", 1));
        fixture.pass_limited(22, 25);
        fixture.pass_limited(23, 25);
        assert_eq!(fixture.state("1"), "cancelled");

        // However many passes go by with the network asking the same, nothing goes round, and no one pays.
        let paid = [fixture.nonce(21), fixture.nonce(22), fixture.nonce(23)];

        for _ in 0..3 {
            fixture.pass(21);
            fixture.pass_limited(22, 25);
            fixture.pass_limited(23, 25);
        }

        assert_eq!(fixture.count("proposal_count"), "1");
        assert_eq!(fixture.withdrawal_state(&id), "queued");
        assert_eq!([fixture.nonce(21), fixture.nonce(22), fixture.nonce(23)], paid);

        // Estimates wandering a little change nothing; a rate well under it is another question.
        fixture.asks(90);
        fixture.pass(21);
        assert_eq!(fixture.count("proposal_count"), "1");
        fixture.asks(70);
        fixture.pass_limited(22, 25);
        assert_eq!(fixture.count("proposal_count"), "1", "still past what this one signs for");
        fixture.pass(21);
        assert_eq!(fixture.count("proposal_count"), "2");
        assert!(fixture.spends("2").contains(&fixture.anchor(&id).unwrap()));
        fixture.pass_limited(22, 25);
        fixture.pass_limited(23, 25);
        assert_eq!(fixture.state("2"), "cancelled");

        // At the same rate again it waits for the limit, by the chain's time, and goes once past it, its
        // next try that far off in turn.
        fixture.pass(21);
        assert_eq!(fixture.count("proposal_count"), "2");
        fixture.pass_past(21, 0);
        assert_eq!(fixture.count("proposal_count"), "3");
        fixture.running.stop();
    }

    /// What says nothing of a release gets no vote: a state read failing, and a limit with no fee rate
    /// behind it. What is refused for good by the same signer a moment later does.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn a_failure_that_may_be_temporary_takes_no_vote() {
        let fixture = StuckReleases::start("btc-temporary");
        fixture.withdraw();
        // About 400 a virtual byte: past even the fixed 200 of a signer with no fee rate.
        fixture.propose_by_hand(23, 80_000);

        // Esplora gone: refused, as it is past the fixed limit, but it may be the going rate.
        let gone = crate::cli::bridge::BridgeArgs { bitcoin: crate::cli::outside::BitcoinArgs { esplora: vec!["http://127.0.0.1:1".to_string()], ..Default::default() }, ..fixture.args(crate::cli::bridge::btc::BtcAction::Pass, Some(21)) };
        crate::cli::bridge::bridge(gone).unwrap();
        assert_eq!((fixture.voted("1", 21), fixture.signed("1")), (false, 0));

        // The node failing what it is asked of a release: nothing is known, so nothing is said.
        let flaky = FlakyNode::start(&fixture.node, "utxo_of");
        let failing = crate::cli::bridge::BridgeArgs { node: flaky.clone(), ..fixture.args(crate::cli::bridge::btc::BtcAction::Pass, Some(21)) };
        crate::cli::bridge::bridge(failing).unwrap();
        assert_eq!((fixture.voted("1", 21), fixture.signed("1")), (false, 0));

        // Its age going unread: no vote for its age, and none lost for the rest.
        let no_index = FlakyNode::start(&fixture.node, "/events");
        let unaged = crate::cli::bridge::BridgeArgs { node: no_index, cancel_after: 0, max_fee_rate: Some(1_000), ..fixture.args(crate::cli::bridge::btc::BtcAction::Pass, Some(22)) };
        crate::cli::bridge::bridge(unaged).unwrap();
        assert_eq!((fixture.voted("1", 22), fixture.signed("1")), (false, 1), "signed, as it stands by what it was told");

        // All of it read again, the same signer votes.
        fixture.pass(21);
        assert!(fixture.voted("1", 21));
        fixture.running.stop();
    }

    /// A release short of its signatures past the age limit takes a vote, from one who signed it too, or
    /// it could not be cancelled once the others are gone. One with all its signatures never does, as
    /// the gateway would refuse it: it is warned about instead.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn past_the_age_limit_a_release_short_of_signatures_is_voted_down_and_one_with_them_is_warned_about() {
        let fixture = StuckReleases::start("btc-age-limit");
        let id = fixture.withdraw();
        // Signer 23 proposed it and went away. It stands, so nothing is refused.
        fixture.propose_by_hand(23, 1_000);
        fixture.pass(21);
        assert_eq!((fixture.signed("1"), fixture.voted("1", 21)), (1, false), "under the limit it is signed as ever");

        // Far under the limit: neither signing nor voting changes for the one who signed.
        let paid = fixture.nonce(21);
        fixture.pass(21);
        assert_eq!(fixture.nonce(21), paid);

        // Past it: no one signs it now, and the one who signed it votes too.
        fixture.pass_past(21, 0);
        assert_eq!((fixture.signed("1"), fixture.voted("1", 21)), (1, true));
        fixture.pass_past(22, 0);
        assert_eq!(fixture.state("1"), "cancelled");
        assert_eq!(fixture.withdrawal_state(&id), "queued");

        // Proposed again at what the network asks now, through its anchor, and this time it is signed.
        let anchor = fixture.anchor(&id).unwrap();
        fixture.asks(2);
        fixture.pass(21);
        assert!(fixture.spends("2").contains(&anchor));
        fixture.pass(22);
        assert_eq!(fixture.signed("2"), 2);

        // Past the limit with all its signatures, broadcast and not mined: a vote would be refused, so none
        // is sent, as none of them pay; and each pass warns, by what the gateway and the network say.
        let posted = fixture.bitcoin.esplora.posted.lock().unwrap().len();
        assert!(posted > 0, "broadcast");
        let txid = FakeBitcoin::display(&crate::structs::hex::from_hex(fixture.proposal("2")[1]["value"].as_str().unwrap()).unwrap());
        fixture.bitcoin.esplora.serve(&format!("/tx/{txid}/hex"), "00".to_string());
        fixture.bitcoin.esplora.serve(&format!("/tx/{txid}/status"), serde_json::json!({ "confirmed": false }).to_string());
        let paid = (fixture.nonce(21), fixture.nonce(22), fixture.nonce(23));
        fixture.pass_past(21, 0);
        fixture.pass_past(22, 0);
        fixture.pass_past(23, 0);
        assert_eq!((fixture.nonce(21), fixture.nonce(22), fixture.nonce(23)), paid, "no one paid to vote or sign");
        assert!(!fixture.voted("2", 21) && !fixture.voted("2", 22) && !fixture.voted("2", 23));
        assert_eq!(fixture.state("2"), "open");

        let bridge = crate::cli::bridge::Bridge::new(crate::cli::bridge::BridgeArgs { cancel_after: 0, ..fixture.args(crate::cli::bridge::btc::BtcAction::Pass, None) }).unwrap();
        let warning = bridge.stuck_warning(&fixture.gateway, 2, fixture.proposal("2").as_array().unwrap(), Some(40)).unwrap().expect("past the limit");
        for part in ["release 2", txid.as_str(), "the network asks 40", "cannot be cancelled", "child-pays-for-parent"] {
            assert!(warning.contains(part), "{part}: {warning}");
        }

        // Under it, nothing.
        let quiet = crate::cli::bridge::Bridge::new(fixture.args(crate::cli::bridge::btc::BtcAction::Pass, None)).unwrap();
        assert_eq!(quiet.stuck_warning(&fixture.gateway, 2, fixture.proposal("2").as_array().unwrap(), Some(40)).unwrap(), None);
        fixture.running.stop();
    }

    /// A node that answers everything as the real one does, but what mentions a word, which it fails.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    struct FlakyNode;

    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    impl FlakyNode {
        /// Its URL, served for as long as the test runs.
        fn start(node: &str, word: &str) -> String {
            use std::io::{BufRead, BufReader, Read, Write};

            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let (node, word) = (node.to_string(), word.to_string());

            std::thread::spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();

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
                    let body = String::from_utf8(body).unwrap();
                    let mut parts = first.split_whitespace();
                    let (method, path) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
                    let (status, answer) = if body.contains(&word) || path.contains(&word) {
                        (500, serde_json::json!({ "error": "this node failed it" }).to_string())
                    } else {
                        let url = format!("{node}{path}");
                        let mut response = match method.as_str() {
                            "POST" => agent.post(&url).header("content-type", "application/json").send(body).unwrap(),
                            _ => agent.get(&url).call().unwrap(),
                        };

                        (response.status().as_u16(), response.body_mut().read_to_string().unwrap())
                    };
                    let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
                }
            });

            url
        }
    }

    /// A transfer refused for good, both ways: voided where it was going, then, once that is final,
    /// refunded where it left, each on the threshold of signers that read both chains for themselves.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "ethereum", feature = "finality-ffg"))]
    #[test]
    fn a_transfer_refused_for_good_is_voided_there_and_only_then_refunded_here_each_way() {
        use crate::abi::{AbiType, AbiValue, decode_values, encode_values};
        use crate::chain::{IndexKind, KeepFrom};
        use crate::cli::bridge::{Bridge, BridgeAction, BridgeArgs, bridge};
        use crate::cli::swap::rpc::{Rpc, from_key};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::registry::federation::Federation;
        use crate::structs::hex::to_hex;
        use crate::structs::tx::TxCallArgumentType;

        let federation = Federation {
            signers: [11u8, 12, 13].iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
            btc: None,
        };
        let finality = FinalityConfig::dev(DevAuthority::dev_keypair().public());
        let indexed = |node: Node| node.keep_index(IndexKind::Events, Some(KeepFrom::Height(0))).unwrap();
        let b = Running::launch_genesis(true, true, |genesis| Genesis { finality: finality.clone(), ..genesis }, |node| node, Default::default());
        let chain_b = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("http://{}/status", b.api)).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["chain_id"].as_u64().unwrap();
        let a = Running::launch_genesis(true, true, |genesis| Genesis { federation: Some(federation.clone()), finality: finality.clone(), ..genesis }, indexed, Default::default());
        let (node_a, rpc_b) = (format!("http://{}", a.api), format!("http://{}/rpc", b.api));
        let dir = TempDir::new("bridge-refund");
        let key_file = |name: &str, secret: [u8; 32]| {
            let path = dir.0.join(name);
            fs::write(&path, Keypair::from_private(PrivateKey::Secp256k1(secret)).to_key_file()).unwrap();
            path
        };
        let relayer = key_file("relayer.key", [0x42; 32]);
        let (signer_one, signer_two) = (key_file("eleven.key", [11; 32]), key_file("twelve.key", [12; 32]));
        let relayer_address = from_key(&[0x42; 32]).unwrap();

        // Each signer takes what the board has enough of over there, and pays its gas.
        for secret in [[0x42; 32], [11; 32], [12; 32]] {
            send(b.args(TxArgs::Transfer { to: Address::Secp256k1(from_key(&secret).unwrap()).into(), amount: u256::from(10u64).pow(20) })).unwrap();
        }

        let Address::Evm(evm_gateway) = crate::vm::evm::create_address(&Address::Secp256k1(relayer_address), 3) else { unreachable!() };
        let vouched = vec![(chain_b, evm_gateway)];
        let me = DevAuthority::dev_keypair().public().address();
        let nonce = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node_a}/accounts/{me}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap();
        let gateway = deployed_at(&me, nonce + 3, crate::structs::hash::HashType::default_chain());
        let args = |action: BridgeAction, evm_key: Option<PathBuf>| BridgeArgs { action, node: node_a.clone(), tls: Default::default(), key: None, gateway: Some(gateway), eth_rpc: vec![rpc_b.clone()], evm_key, btc_gateway: None, bitcoin: Default::default(), beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(), evm_gateways: vouched.clone(), max_fee_rate: None, cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS, bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS };
        let run = |action: BridgeAction, evm_key: Option<PathBuf>| bridge(args(action, evm_key));
        bridge(BridgeArgs { gateway: None, ..args(BridgeAction::Deploy, None) }).unwrap();
        run(BridgeAction::DeployEvm { coin_name: "Dev".into(), coin_symbol: "DEV".into() }, Some(relayer.clone())).unwrap();
        run(BridgeAction::Peer { evm_gateway: to_hex(&evm_gateway) }, Some(relayer.clone())).unwrap();
        let rpc = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &rpc_b);
        let wait = |what: &str, done: &mut dyn FnMut() -> bool| {
            let started = std::time::Instant::now();

            while !done() {
                assert!(started.elapsed() < std::time::Duration::from_secs(90), "{what} never happened");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        };
        let read = |function: &str, values: serde_json::Value| -> Vec<serde_json::Value> {
            let body = serde_json::json!({ "contract": gateway.to_string(), "function": function, "args": values });
            serde_json::from_str::<serde_json::Value>(&ureq::post(format!("{node_a}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap()["values"].as_array().cloned().unwrap_or_default()
        };
        let there = |signature: &str, values: &[AbiValue], returns: &[AbiType]| {
            let (_, types) = crate::abi::parse_signature(signature).unwrap();
            let input = [crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::Keccak256, signature.as_bytes()).to_bytes()[..4].to_vec(), encode_values(&types, values).unwrap()].concat();

            decode_values(returns, &rpc.call(&evm_gateway, &input).unwrap()).unwrap()
        };
        let refund = |nonce: u64, from_evm: bool, key: &PathBuf| run(BridgeAction::Refund { nonce, from_evm, why: None, yes: true }, Some(key.clone()));
        let at = |source: u64, nonce: u64| serde_json::json!([{ "type": "u64", "value": source.to_string() }, { "type": "u64", "value": nonce.to_string() }]);

        // --- Leaving there for here. ---
        // 1,000 of ETH left Gateway.sol for Bob here, and it is decided no delivery shall come.
        let bob = Address::Ed25519([0xb0; 32]);
        run(BridgeAction::Send { kind: "coin".into(), recipient: bob.to_string(), amount: u256::new(1_000), asset: None, id: u256::ZERO, from_evm: true }, Some(relayer.clone())).unwrap();
        assert_eq!(rpc.balance(&evm_gateway).unwrap(), u256::new(1_000));

        // One signer's word for its void, as soon as it left in a final block, and an operator saying no
        // to the question stops it.
        wait("it being final where it left", &mut || refund(1, true, &signer_one).is_ok());
        let refused = Bridge::new(args(BridgeAction::Relay, Some(signer_two.clone()))).unwrap().refund(1, true, None, false, &mut "no\n".as_bytes());
        assert_eq!(refused.unwrap_err().to_string(), "nothing is signed");
        assert!(read("voided", at(chain_b, 1))[0]["value"].is_null(), "one signer voids nothing, and no one who said no");
        let balance_before = rpc.balance(&from_key(&[0x42; 32]).unwrap()).unwrap();

        // A second: voided here. Delivery signatures posted now are refused, by a signer's pass too.
        refund(1, true, &signer_two).unwrap();
        assert!(!read("voided", at(chain_b, 1))[0]["value"].is_null());
        run(BridgeAction::Sign { from_block: 0 }, Some(signer_one.clone())).unwrap();
        assert_eq!(read("received", at(chain_b, 1))[0]["value"], false);

        // Once that is final, each signs to refund it, and whoever makes the threshold takes it there.
        wait("the refund", &mut || {
            for key in [&signer_one, &signer_two] {
                refund(1, true, key).unwrap();
            }

            there("refunded(uint64)", &[AbiValue::Uint(u256::new(1))], &[AbiType::Bool]) == [AbiValue::Bool(true)]
        });
        assert_eq!(rpc.balance(&evm_gateway).unwrap(), u256::ZERO);
        assert_eq!(rpc.balance(&from_key(&[0x42; 32]).unwrap()).unwrap(), balance_before + u256::new(1_000), "to who sent it, who paid no gas for this");

        // Run again, it says so, and pays nothing.
        refund(1, true, &signer_one).unwrap();
        assert_eq!(rpc.balance(&evm_gateway).unwrap(), u256::ZERO);

        // --- Leaving here for there. ---
        // Wrapped ETH reaches the dev key here, delivered as usual.
        run(BridgeAction::Send { kind: "coin".into(), recipient: me.to_string(), amount: u256::new(2_000), asset: None, id: u256::ZERO, from_evm: true }, Some(relayer.clone())).unwrap();
        let wrapped = || read("wrapped_of", serde_json::json!([{ "type": "u64", "value": chain_b.to_string() }, { "type": "u64", "value": "0" }, { "type": "bytes", "value": "0x" }]));
        wait("the deposit", &mut || {
            for key in [&signer_one, &signer_two] {
                run(BridgeAction::Sign { from_block: 0 }, Some(key.clone())).unwrap();
            }

            wrapped().first().is_some_and(|value| !value["value"].is_null())
        });
        let weth: Address = wrapped()[0]["value"].as_str().unwrap().parse().unwrap();
        let held = || {
            let body = serde_json::json!({ "contract": weth.to_string(), "function": "balance_of", "args": [{ "type": "address", "value": me.to_string() }] });
            let answer: serde_json::Value = serde_json::from_str(&ureq::post(format!("{node_a}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap();

            answer["values"][0]["value"].as_str().unwrap().to_string()
        };
        assert_eq!(held(), "2000");

        // 500 back to Gateway.sol itself, which takes no coin: burned here, and refused there for good.
        let send_back = |amount: u64, recipient: &[u8; 20]| {
            send(a.args(TxArgs::Call { to: gateway, function: "send".to_string(), amount: u256::ZERO, args: vec![TxCallArgumentType::U64(chain_b), TxCallArgumentType::Bytes(recipient.to_vec()), TxCallArgumentType::U64(1), TxCallArgumentType::Address(weth), TxCallArgumentType::U256(u256::ZERO), TxCallArgumentType::U256(u256::new(u128::from(amount)))] })).unwrap();
        };
        send_back(500, &evm_gateway);
        assert_eq!(held(), "1500");
        let posted_for = |nonce: u64| {
            let digest = read("sent", serde_json::json!([{ "type": "u64", "value": nonce.to_string() }]))[11]["value"].clone();

            read("signatures", serde_json::json!([{ "type": "bytes", "value": digest }])).len() - 1
        };
        wait("its delivery being signed", &mut || {
            for key in [&signer_one, &signer_two] {
                run(BridgeAction::Sign { from_block: 0 }, Some(key.clone())).unwrap();
            }

            posted_for(1) >= 2
        });

        // A transfer that would be taken is not refunded: it is delivered, and then there is nothing to refund.
        send_back(100, &[0xcd; 20]);
        wait("that one being signed too", &mut || {
            for key in [&signer_one, &signer_two] {
                run(BridgeAction::Sign { from_block: 0 }, Some(key.clone())).unwrap();
            }

            posted_for(2) >= 2
        });
        let would = refund(2, false, &signer_one).unwrap_err().to_string();
        assert!(would.contains("would be taken"), "{would}");
        run(BridgeAction::Relay, Some(signer_one.clone())).unwrap();
        let delivered = refund(2, false, &signer_one).unwrap_err().to_string();
        assert!(delivered.contains("was delivered"), "{delivered}");

        // The other, refused there, is voided there: each signer's word goes on the board, and the one
        // that makes the threshold takes them over. Its delivery signatures, public as they are, then fail.
        refund(1, false, &signer_one).unwrap();
        refund(1, false, &signer_two).unwrap();
        let here = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node_a}/status")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["chain_id"].as_u64().unwrap();
        let identity = read("identity", serde_json::json!([]))[0]["value"].as_str().and_then(crate::structs::hex::from_hex).unwrap();
        let voided_key = crate::bridge::Transfer { from: identity, to: evm_gateway.to_vec(), source: here, target: chain_b, nonce: 1, kind: crate::bridge::AssetKind::Coin, origin: chain_b, asset: Vec::new(), id: u256::ZERO, amount: u256::ZERO, recipient: Vec::new(), name: String::new(), symbol: String::new(), decimals: 0, uri: String::new() }.delivered_key();
        assert_ne!(there("voidedAt(bytes32)", &[AbiValue::FixedBytes(voided_key.clone())], &[AbiType::Uint(256)]), [AbiValue::Uint(u256::ZERO)]);
        assert_eq!(there("delivered(bytes32)", &[AbiValue::FixedBytes(voided_key)], &[AbiType::Bool]), [AbiValue::Bool(false)]);
        run(BridgeAction::Relay, Some(signer_one.clone())).unwrap();

        // Once final there, the threshold's refund mints it back here, once.
        wait("the refund here", &mut || {
            for key in [&signer_one, &signer_two] {
                refund(1, false, key).unwrap();
            }

            read("refund_of", serde_json::json!([{ "type": "u64", "value": "1" }]))[1]["value"] == true
        });
        assert_eq!(held(), "1900", "the 500 back, and the 100 that was delivered gone");
        refund(1, false, &signer_one).unwrap();

        // Its owner repoints the gateway's peer, and a signer's operator vouches for the new one: what
        // left for the old is no business of that contract's, whose word could say anything.
        send_back(50, &[0xcd; 20]);
        let repointed = [0xfa; 20];
        send(a.args(TxArgs::Call { to: gateway, function: "set_peer".to_string(), amount: u256::ZERO, args: vec![TxCallArgumentType::U64(chain_b), TxCallArgumentType::Bytes(repointed.to_vec())] })).unwrap();
        let word = bridge(BridgeArgs { evm_gateways: vec![(chain_b, repointed)], ..args(BridgeAction::Refund { nonce: 3, from_evm: false, why: None, yes: true }, Some(signer_one.clone())) }).unwrap_err().to_string();
        assert!(word.contains("did not leave for the gateway named"), "{word}");
        a.stop();
        b.stop();
    }

    /// Signers leaving their passes running: what is queued is proposed, once, however many of them
    /// go at it, signed, broadcast and shown, with no one choosing a fee or anything else. What cannot
    /// go waits and says why, and what costs too much is passed over, by the network's word or, with
    /// Esplora gone, a fixed limit.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "lvm", feature = "wasm"))]
    #[test]
    fn withdrawals_are_proposed_signed_and_paid_by_the_signers_own_passes() {
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
            btc: None,
        };
        let me = DevAuthority::dev_keypair().public().address();
        let hash = crate::structs::hash::HashType::default_chain();
        let announced = deployed_at(&me, 3, hash);
        let directory = crate::registry::directory::Directory { btc_gateway: Some(announced), ..Default::default() };
        let running = Running::launch_genesis(true, false, |genesis| Genesis { federation: Some(federation.clone()), directory: Some(directory.clone()), ..genesis }, |node| node, Default::default());
        let node = format!("http://{}", running.api);
        let mut bitcoin = FakeBitcoin::start();
        let dir = TempDir::new("btc-passes");
        let esplora = bitcoin.esplora.url.clone();
        let nonce = |of: &Address| -> u64 { serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node}/accounts/{of}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap() };
        let read = |contract: &Address, function: &str, args: serde_json::Value| -> serde_json::Value {
            let body = serde_json::json!({ "contract": contract.to_string(), "function": function, "args": args });
            serde_json::from_str::<serde_json::Value>(&ureq::post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap()["values"].clone()
        };
        // Each signer pays its own posts with its own key, as signers do.
        let signer = |secret: u8| -> PathBuf {
            let key = dir.0.join(format!("signer-{secret}.key"));

            if !key.exists() {
                fs::write(&key, Keypair::from_private(PrivateKey::Secp256k1([secret; 32])).to_key_file()).unwrap();
            }

            key
        };
        let payer = |secret: u8| Keypair::from_private(PrivateKey::Secp256k1([secret; 32])).public().address();

        for secret in [21u8, 22, 23] {
            send(running.args(TxArgs::Transfer { to: payer(secret).into(), amount: u256::from(10u64).pow(20) })).unwrap();
        }

        let wasm = dir.0.join("btc_relay.wasm");
        fs::write(&wasm, crate::vm::tests::example("btc_relay")).unwrap();
        let start = nonce(&me);
        assert_eq!(start, 3, "the signers paid for, so the gateway is deployed past them");
        let relay = deployed_at(&me, start, hash);
        send(running.args(TxArgs::Deploy { lua: None, wasm: Some(wasm), evm: None, constructor: None, constructor_args: Vec::new() })).unwrap();
        let args = |action: BtcAction, evm_key: Option<PathBuf>, key: Option<PathBuf>, esplora: &str, btc_gateway: Option<Address>, max_fee_rate: Option<u64>| BridgeArgs {
            action: BridgeAction::Btc { action },
            node: node.clone(),
            tls: Default::default(),
            key,
            gateway: None,
            eth_rpc: Vec::new(),
            evm_key,
            btc_gateway,
            bitcoin: crate::cli::outside::BitcoinArgs { esplora: vec![esplora.to_string()], ..Default::default() },
            beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(),
            evm_gateways: Vec::new(),
            max_fee_rate,
            cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS,
            bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS,
        };
        let run = |action: BtcAction, gateway: Option<Address>| bridge(args(action, None, None, &esplora, gateway, None));
        run(BtcAction::RelayStart { relay, height: 0, network: Some("regtest".into()) }, None).unwrap();
        let gateway = deployed_at(&me, start + 3, hash);
        run(BtcAction::Deploy { relay, fee_cap: 100_000, fee_floor: 2, grace_hours: 1, least_deposit: 10_000 }, Some(gateway)).unwrap();
        let output = read(&gateway, "deposit_script", serde_json::json!([{ "type": "address", "value": me.to_string() }]))[1]["value"].as_str().map(|hex| crate::structs::hex::from_hex(hex).unwrap()).unwrap();

        // What the gateway holds: three deposits to spend on all that follows, each release holding one.
        for sats in [300_000, 300_001, 300_002] {
            let deposit = Transaction { version: 2, inputs: vec![TxIn { txid: [0x5e; 32], vout: 0, script_sig: Vec::new(), sequence: 0xffff_ffff, witness: Vec::new() }], outputs: vec![TxOut { value: sats, script: output.clone() }], lock_time: 0 }.base();
            bitcoin.block(std::slice::from_ref(&deposit), false);
            bitcoin.block(&[], false);
            run(BtcAction::RelaySync { relay, batch: 10 }, Some(gateway)).unwrap();
            run(BtcAction::Deposit { txid: FakeBitcoin::display(&crate::crypto::bitcoin::sha256d(&deposit)), vout: 0, recipient: me }, Some(gateway)).unwrap();
        }

        bitcoin.esplora.serve("/fee-estimates", r#"{"1":5.0,"3":4.2,"6":2.0}"#.to_string());
        let to = crate::structs::bech32::encode_segwit("bcrt", &[0xb0; 20]);
        let withdraw = |sats: u64, most_fee: Option<u64>| run(BtcAction::Withdraw { address: to.clone(), sats, most_fee, network: None }, Some(gateway)).unwrap();
        let state_of = |id: &str| read(&gateway, "withdrawal", serde_json::json!([{ "type": "u64", "value": id }]))[2]["value"].as_str().unwrap().to_string();
        let count = || read(&gateway, "proposal_count", serde_json::json!([]))[0]["value"].as_str().unwrap().to_string();
        let signed = |proposal: &str| (read(&gateway, "input_signatures", serde_json::json!([{ "type": "u64", "value": proposal }, { "type": "u64", "value": "0" }])).as_array().unwrap().len() - 2) / 2;
        let pass = |secret: u8| bridge(args(BtcAction::Pass, Some(signer(secret)), Some(signer(secret)), &esplora, Some(gateway), None)).map_err(|err| err.to_string());

        // Withdrawal 1 agreed to lose next to nothing: it must not hold up 2.
        withdraw(10_000, Some(1));
        withdraw(20_000, None);

        // A stranger's key passes without proposing or signing anything.
        bridge(args(BtcAction::Pass, Some(signer(99)), None, &esplora, Some(gateway), None)).unwrap();
        assert_eq!(count(), "0");
        bridge(args(BtcAction::Pass, None, None, &esplora, Some(gateway), None)).unwrap();
        assert_eq!(count(), "0", "nor does a pass with no signer's key");

        // Two signers in the same pass, each building the same one: one release, no one failing.
        let passes: Vec<Result<(), String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = [21u8, 22].map(|secret| {
                let pass = &pass;
                scope.spawn(move || pass(secret))
            }).into_iter().collect();
            handles.into_iter().map(|handle| handle.join().unwrap()).collect()
        });
        assert_eq!(passes, [Ok(()), Ok(())]);
        assert_eq!(count(), "1", "the first won, and the other found that");
        assert_eq!(state_of("1"), "queued");
        assert_eq!(state_of("2"), "proposed");
        let found = read(&gateway, "proposal", serde_json::json!([{ "type": "u64", "value": "1" }]));
        let (release, _) = Transaction::read(&crate::structs::hex::from_hex(found[0]["value"].as_str().unwrap()).unwrap()).unwrap();
        // At the rate the network asks for soon, rounded up: 5 satoshis a virtual byte.
        let vbytes = crate::bridge::btc_release_vbytes(release.base().len(), &[(2, read(&gateway, "utxo_of", serde_json::json!([{ "type": "string", "value": read(&gateway, "utxo", serde_json::json!([{ "type": "u64", "value": "0" }]))[0]["value"].as_str().unwrap() }]))[4]["value"].as_str().unwrap().len() / 2 - 1)]);
        assert_eq!(release.outputs[0].value, 20_000 - vbytes * 5);

        // Whichever went first, another pass signs what the other had not seen, and broadcasts it.
        pass(21).unwrap();
        pass(22).unwrap();
        assert_eq!(signed("1"), 2);
        assert!(!bitcoin.esplora.posted.lock().unwrap().is_empty(), "with its signatures, it is broadcast");
        let (mined, witnessed) = Transaction::read(&crate::structs::hex::from_hex(&format!("0x{}", bitcoin.esplora.posted.lock().unwrap().last().unwrap())).unwrap()).unwrap();
        assert!(witnessed);

        // Mined, buried and relayed: a pass shows it, and the withdrawal is paid.
        bitcoin.block(std::slice::from_ref(&mined.base()), false);
        bitcoin.block(&[], false);
        run(BtcAction::RelaySync { relay, batch: 10 }, Some(gateway)).unwrap();
        pass(23).unwrap();
        assert_eq!(state_of("2"), "paid");

        // With nothing more it can do, a pass pays for nothing, however often it goes.
        let before = (nonce(&payer(21)), nonce(&payer(22)));
        let shown = bitcoin.esplora.posted.lock().unwrap().len();
        pass(21).unwrap();
        pass(22).unwrap();
        assert_eq!((nonce(&payer(21)), nonce(&payer(22))), before);
        assert_eq!(bitcoin.esplora.posted.lock().unwrap().len(), shown);
        assert_eq!(state_of("1"), "queued", "and what cannot go stays, until it may");

        // Raised, it goes on the next pass. A signer that worked out the same release a moment too late
        // finds it beaten: no failure, and nothing paid, as the dry run finds it first.
        run(BtcAction::SetMostFee { withdrawal: 1, most_fee: 2_000 }, Some(gateway)).unwrap();
        let late = crate::cli::bridge::Bridge::new(args(BtcAction::Pass, Some(signer(22)), Some(signer(22)), &esplora, Some(gateway), None)).unwrap();
        let planned = late.plan_queue(&gateway, crate::bridge::ReleaseFee::Rate(5)).unwrap().unwrap();
        let tx = planned.tx.clone().unwrap();
        // A stranger's word is a real failure, with no one having come first.
        assert!(late.post_unless_beaten(&gateway, &[99; 32], &tx, &planned.ids, 1).unwrap_err().to_string().contains("only a signer"));
        let paid_before = nonce(&payer(22));
        pass(21).unwrap();
        assert_eq!((count().as_str(), state_of("1").as_str()), ("2", "proposed"));
        assert!(!late.post_unless_beaten(&gateway, &[22; 32], &tx, &planned.ids, 1).unwrap(), "beaten to it");
        assert_eq!(nonce(&payer(22)), paid_before);
        assert_eq!(count(), "2");

        // A release that pays far too much, by a signer gone astray: at 5 fastest the others sign for up
        // to 25 a virtual byte, and 20,000 is about 100 of them. Signing by hand, its operator may say
        // otherwise. (What a pass does about it is shown below, in what votes it down.)
        let hand = |fee: u64| bridge(args(BtcAction::Propose { fee: Some(fee) }, Some(signer(23)), Some(signer(23)), &esplora, Some(gateway), None)).unwrap();
        withdraw(200_000, Some(100_000));
        hand(20_000);
        assert_eq!(count(), "3");
        bridge(args(BtcAction::Sign, Some(signer(22)), Some(signer(22)), &esplora, Some(gateway), None)).unwrap();
        assert_eq!(signed("3"), 0, "refused by hand, logged, and no vote");
        bridge(args(BtcAction::Sign, Some(signer(21)), Some(signer(21)), &esplora, Some(gateway), Some(200))).unwrap();
        assert_eq!(signed("3"), 1);

        // Esplora gone: what is queued stays queued, a pass stops for nothing, and what stands under the
        // fixed limit is signed, as fees going unread should not halt withdrawals.
        let gone = "http://127.0.0.1:1";
        let blind = |secret: u8| bridge(args(BtcAction::Pass, Some(signer(secret)), Some(signer(secret)), gone, Some(gateway), None));
        blind(22).unwrap();
        assert_eq!(signed("3"), 2, "about 100 a virtual byte, under the fixed 200");
        withdraw(200_000, Some(100_000));
        blind(22).unwrap();
        assert_eq!(state_of("4"), "queued", "no rate to propose at");
        assert_eq!(count(), "3");
        // Past the fixed limit it is passed over all the same.
        hand(80_000);
        assert_eq!(count(), "4");
        blind(21).unwrap();
        assert_eq!(signed("4"), 0, "about 400 a virtual byte");
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

        // Chain A runs the gateway here and the federation, and lists Gateway.sol in its directory;
        // chain B's EVM is the other chain.
        let federation = Federation {
            signers: [11u8, 12, 13].iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
            btc: None,
        };
        let finality = FinalityConfig::dev(DevAuthority::dev_keypair().public());
        let indexed = |node: Node| node.keep_index(IndexKind::Events, Some(KeepFrom::Height(0))).unwrap();
        let b = Running::launch_genesis(true, true, |genesis| Genesis { finality: finality.clone(), ..genesis }, |node| node, Default::default());
        let chain_b = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("http://{}/status", b.api)).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["chain_id"].as_u64().unwrap();
        let listed_gateway = match crate::vm::evm::create_address(&Address::Secp256k1(from_key(&[0x42; 32]).unwrap()), 3) {
            Address::Evm(gateway) => gateway,
            _ => unreachable!(),
        };
        let directory = crate::registry::directory::Directory { evm_gateways: vec![crate::registry::directory::EvmGateway { chain_id: chain_b, gateway: listed_gateway }], ..Default::default() };
        let a = Running::launch_genesis(true, true, |genesis| Genesis { federation: Some(federation.clone()), directory: Some(directory.clone()), finality: finality.clone(), ..genesis }, indexed, Default::default());
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
            bridge(BridgeArgs { action, node: node_a.clone(), tls: Default::default(), key: None, gateway: None, eth_rpc: vec![rpc_b.clone()], evm_key, btc_gateway: None, bitcoin: Default::default(), beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(), evm_gateways: Vec::new(), max_fee_rate: None, cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS, bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS })
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
            bridge(BridgeArgs { action, node: node_a.clone(), tls: Default::default(), key: None, gateway: Some(gateway), eth_rpc: vec![rpc_b.clone()], evm_key, btc_gateway: None, bitcoin: Default::default(), beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(), evm_gateways: Vec::new(), max_fee_rate: None, cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS, bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS })
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

    /// The owner of the gateway here names a peer of its own on a chain the directory leaves out:
    /// one key, and no signer may take its word for what left there.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "ethereum", feature = "finality-ffg"))]
    #[test]
    fn no_signer_vouches_for_a_chain_neither_the_directory_nor_its_operator_names() {
        use crate::chain::{IndexKind, KeepFrom};
        use crate::cli::bridge::{Bridge, BridgeAction, BridgeArgs, bridge};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::registry::federation::Federation;
        use crate::structs::tx::TxCallArgumentType;

        const FAKE: [u8; 20] = [0xfa; 20];
        const CHAIN: u64 = 777;
        let federation = Federation { signers: vec![crate::crypto::secp256k1::public_key(&[11; 32]).unwrap()], threshold: 1, chain_depth: 20, evm_depth: 12, bitcoin_depth: 6, btc: None };
        let finality = FinalityConfig::dev(DevAuthority::dev_keypair().public());
        let a = Running::launch_genesis(true, true, |genesis| Genesis { federation: Some(federation.clone()), finality: finality.clone(), ..genesis }, |node: Node| node.keep_index(IndexKind::Events, Some(KeepFrom::Height(0))).unwrap(), Default::default());
        let node = format!("http://{}", a.api);
        let dir = TempDir::new("bridge-unlisted");
        let key = dir.0.join("signer.key");
        fs::write(&key, Keypair::from_private(PrivateKey::Secp256k1([11; 32])).to_key_file()).unwrap();
        let me = DevAuthority::dev_keypair().public().address();
        let nonce = serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node}/accounts/{me}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap();
        let gateway = deployed_at(&me, nonce + 3, crate::structs::hash::HashType::default_chain());
        let args = |action: BridgeAction, evm_gateways: Vec<(u64, [u8; 20])>| BridgeArgs { action, node: node.clone(), tls: Default::default(), key: None, gateway: Some(gateway), eth_rpc: Vec::new(), evm_key: Some(key.clone()), btc_gateway: None, bitcoin: Default::default(), beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(), evm_gateways, max_fee_rate: None, cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS, bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS };
        bridge(BridgeArgs { gateway: None, ..args(BridgeAction::Deploy, Vec::new()) }).unwrap();

        // Its owner's own peer, and a coin sent to it.
        let call = |function: &str, amount: u256, values: Vec<TxCallArgumentType>| send(a.args(TxArgs::Call { to: gateway, function: function.to_string(), amount, args: values })).unwrap();
        call("set_peer", u256::ZERO, vec![TxCallArgumentType::U64(CHAIN), TxCallArgumentType::Bytes(FAKE.to_vec())]);
        call("send", u256::new(5), vec![TxCallArgumentType::U64(CHAIN), TxCallArgumentType::Bytes(vec![0xb0; 20]), TxCallArgumentType::U64(0), TxCallArgumentType::Nil, TxCallArgumentType::U256(u256::ZERO), TxCallArgumentType::U256(u256::new(5))]);
        let posted = || {
            let body = serde_json::json!({ "contract": gateway.to_string(), "function": "sent", "args": [{ "type": "u64", "value": "1" }] });
            let sent: serde_json::Value = serde_json::from_str(&ureq::post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap();
            let digest = sent["values"][11]["value"].clone();
            let body = serde_json::json!({ "contract": gateway.to_string(), "function": "signatures", "args": [{ "type": "bytes", "value": digest }] });
            let answer: serde_json::Value = serde_json::from_str(&ureq::post(format!("{node}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap();

            answer["values"].as_array().unwrap().len() - 1
        };
        let signing = |evm_gateways: Vec<(u64, [u8; 20])>| {
            let signer = Bridge::new(args(BridgeAction::Relay, evm_gateways)).unwrap();
            let started = std::time::Instant::now();

            // Until the event is final, nothing is even looked at.
            loop {
                signer.pass(0);

                if !signer.refused_now().is_empty() || posted() > 0 || started.elapsed() > std::time::Duration::from_secs(20) {
                    return signer.refused_now();
                }

                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        };

        // No directory at all, and no word of its own: refused, and it says what would do.
        let refused = signing(Vec::new());
        assert!(refused.iter().any(|line| line.contains("lists no gateway on chain 777") && line.contains("--evm-gateway")), "{refused:?}");
        assert_eq!(posted(), 0);

        // Its operator names another: refused too, so the flag does not bless whatever the owner set.
        let refused = signing(vec![(CHAIN, [0xbb; 20])]);
        assert!(refused.iter().any(|line| line.contains("--evm-gateway names")), "{refused:?}");
        assert_eq!(posted(), 0);

        // Only its operator's own word for that very gateway signs.
        assert_eq!(signing(vec![(CHAIN, FAKE)]), Vec::<String>::new());
        assert_eq!(posted(), 1);
        a.stop();
    }

    /// Everything between the gateways going wrong as it may in the field: a recipient that could
    /// never arrive, a token whose name is not text, a gateway set up anew with Gateway.sol's logs
    /// still naming the last one, and a transfer that gateway must refuse, each passed over for what
    /// comes next, signing and relaying alike.
    #[cfg(all(feature = "bridge", feature = "swap", feature = "ethereum", feature = "finality-ffg"))]
    #[test]
    fn a_transfer_refused_stops_neither_the_next_one_nor_the_relay_and_old_logs_are_passed_over() {
        use crate::abi::{AbiType, AbiValue, decode_values, encode_values};
        use crate::bridge::{AssetKind, send_input};
        use crate::chain::{IndexKind, KeepFrom};
        use crate::cli::bridge::{Bridge, BridgeAction, BridgeArgs, bridge};
        use crate::cli::swap::rpc::{Rpc, from_key};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::registry::federation::Federation;
        use crate::structs::hex::{from_hex, to_hex};
        use crate::swap::evm::Call;

        let federation = Federation {
            signers: [11u8, 12].iter().map(|byte| crate::crypto::secp256k1::public_key(&[*byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
            btc: None,
        };
        let finality = FinalityConfig::dev(DevAuthority::dev_keypair().public());
        let indexed = |node: Node| node.keep_index(IndexKind::Events, Some(KeepFrom::Height(0))).unwrap();
        let a = Running::launch_genesis(true, true, |genesis| Genesis { federation: Some(federation.clone()), finality: finality.clone(), ..genesis }, indexed, Default::default());
        let b = Running::launch_genesis(true, true, |genesis| Genesis { finality: finality.clone(), ..genesis }, |node| node, Default::default());
        let (node_a, rpc_b) = (format!("http://{}", a.api), format!("http://{}/rpc", b.api));
        let dir = TempDir::new("bridge-refused");
        let key_file = |name: &str, secret: [u8; 32]| {
            let path = dir.0.join(name);
            fs::write(&path, Keypair::from_private(PrivateKey::Secp256k1(secret)).to_key_file()).unwrap();
            path
        };
        let relayer = key_file("relayer.key", [0x42; 32]);
        let (signer_one, signer_two) = (key_file("eleven.key", [11; 32]), key_file("twelve.key", [12; 32]));
        let relayer_address = from_key(&[0x42; 32]).unwrap();

        // Each signer relays too, as watch does, and pays the gas with its key.
        for secret in [[0x42; 32], [11; 32], [12; 32]] {
            send(b.args(TxArgs::Transfer { to: Address::Secp256k1(from_key(&secret).unwrap()).into(), amount: u256::from(10u64).pow(20) })).unwrap();
        }

        let rpc = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &rpc_b);
        // This chain has no directory, so each signer vouches for Gateway.sol itself.
        let Address::Evm(evm_gateway) = crate::vm::evm::create_address(&Address::Secp256k1(relayer_address), 3) else { unreachable!() };
        let vouched = vec![(rpc.chain_id().unwrap(), evm_gateway)];
        let args = |action: BridgeAction, gateway: Option<Address>, evm_key: Option<PathBuf>| BridgeArgs { action, node: node_a.clone(), tls: Default::default(), key: None, gateway, eth_rpc: vec![rpc_b.clone()], evm_key, btc_gateway: None, bitcoin: Default::default(), beacon: Default::default(), proxy: Default::default(), eth_auth: Default::default(), evm_gateways: vouched.clone(), max_fee_rate: None, cancel_after: crate::bridge::BTC_CANCEL_AFTER_MS, bump_after: crate::registry::federation::BtcPolicy::BUMP_AFTER_MS };
        let wait = |what: &str, done: &mut dyn FnMut() -> bool| {
            let started = std::time::Instant::now();

            while !done() {
                assert!(started.elapsed() < std::time::Duration::from_secs(60), "{what} never happened");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        };
        let status = |node: &str| serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node}/status")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap();
        let here = status(&node_a)["chain_id"].as_u64().unwrap();
        let me = DevAuthority::dev_keypair().public().address();
        let hash = crate::structs::hash::HashType::default_chain();
        let nonce_of = || serde_json::from_str::<serde_json::Value>(&ureq::get(format!("{node_a}/accounts/{me}")).call().unwrap().body_mut().read_to_string().unwrap()).unwrap()["nonce"].as_u64().unwrap();
        let read = |contract: &Address, function: &str, values: serde_json::Value| -> serde_json::Value {
            let body = serde_json::json!({ "contract": contract.to_string(), "function": function, "args": values });
            serde_json::from_str::<serde_json::Value>(&ureq::post(format!("{node_a}/call")).header("content-type", "application/json").send(body.to_string()).unwrap().body_mut().read_to_string().unwrap()).unwrap()["values"].clone()
        };
        let evm_read = |to: &[u8; 20], signature: &str, values: &[AbiValue], returns: &[AbiType]| {
            let (_, types) = crate::abi::parse_signature(signature).unwrap();
            let input = [crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::Keccak256, signature.as_bytes()).to_bytes()[..4].to_vec(), encode_values(&types, values).unwrap()].concat();
            decode_values(returns, &rpc.call(to, &input).unwrap()).unwrap()
        };

        // The gateway here, Gateway.sol there, each the other's peer, as its identity.
        let gateway = deployed_at(&me, nonce_of() + 3, hash);
        bridge(args(BridgeAction::Deploy, None, None)).unwrap();
        bridge(args(BridgeAction::DeployEvm { coin_name: "Dev".into(), coin_symbol: "DEV".into() }, Some(gateway), Some(relayer.clone()))).unwrap();
        bridge(args(BridgeAction::Peer { evm_gateway: to_hex(&evm_gateway) }, Some(gateway), Some(relayer.clone()))).unwrap();

        // One wei for a recipient that could never arrive: Gateway.sol takes it no more, whichever
        // way it is wrong, and takes an address that is one.
        let send_wei = |recipient: &[u8]| rpc.send(&Call { to: evm_gateway, value: u256::new(1), input: send_input(here, recipient, AssetKind::Coin, [0; 20], u256::ZERO, u256::new(1)), what: "Send one wei" }, false, &[0x42; 32]);
        let bob = Address::Ed25519([0xb0; 32]);
        let mut unknown = bob.to_abi();
        unknown[0] = 6;
        let mut unpadded = Address::Evm([1; 20]).to_abi();
        unpadded[32] = 1;

        for wrong in [&[0xde; 5][..], &[0xde; 20], &[0xde; 34], &unknown, &unpadded, &[0; 33]] {
            let refused = send_wei(wrong).unwrap_err().to_string();
            assert!(refused.contains("no address on that chain"), "{refused}");
        }

        // A token whose name and symbol are not text crosses without them, and is signed.
        let token_abi = |name: &[u8], supply: u64| encode_values(&[AbiType::Bytes, AbiType::Bytes, AbiType::Uint(256)], &[AbiValue::Bytes(name.to_vec()), AbiValue::Bytes(name.to_vec()), AbiValue::Uint(u256::from(supply))]).unwrap();
        let token_code = from_hex(include_str!("../../contracts/evm/Token.bin").trim()).unwrap();
        let deploy_token = |name: &[u8]| {
            let init = [token_code.clone(), token_abi(name, 1_000)].concat();
            let receipt = rpc.send(&Call { to: [0; 20], value: u256::ZERO, input: init, what: "Deploy a token" }, true, &[0x42; 32]).unwrap();
            <[u8; 20]>::try_from(from_hex(receipt["contractAddress"].as_str().unwrap()).unwrap()).unwrap()
        };
        let unreadable = deploy_token(&[0xff, 0xfe, 0x41]);
        let send_token = |token: [u8; 20], amount: u64, gateway: Address| {
            bridge(args(BridgeAction::Send { kind: "token".into(), recipient: bob.to_string(), amount: u256::from(amount), asset: Some(to_hex(&token)), id: u256::ZERO, from_evm: true }, Some(gateway), Some(relayer.clone()))).unwrap();
        };
        send_token(unreadable, 10, gateway);

        // Signers that keep what they were refused, pass after pass, as watch does.
        let one = Bridge::new(args(BridgeAction::Relay, Some(gateway), Some(signer_one.clone()))).unwrap();
        let two = Bridge::new(args(BridgeAction::Relay, Some(gateway), Some(signer_two.clone()))).unwrap();
        let wrapped_here = |gateway: &Address, token: &[u8; 20]| -> Option<Address> {
            read(gateway, "wrapped_of", serde_json::json!([{ "type": "u64", "value": rpc.chain_id().unwrap().to_string() }, { "type": "u64", "value": "1" }, { "type": "bytes", "value": to_hex(token) }]))[0]["value"].as_str().and_then(|text| text.parse().ok())
        };
        wait("a token named in bytes that are not text", &mut || {
            one.pass(0);
            two.pass(0);
            wrapped_here(&gateway, &unreadable).is_some()
        });
        let held_here = |token: &Address, holder: &Address| read(token, "balance_of", serde_json::json!([{ "type": "address", "value": holder.to_string() }]))[0]["value"].as_str().unwrap().to_string();
        assert_eq!(held_here(&wrapped_here(&gateway, &unreadable).unwrap(), &bob), "10");

        // 1,000 of the coin to Gateway.sol's chain, delivered by the same passes.
        bridge(args(BridgeAction::Send { kind: "coin".into(), recipient: to_hex(&relayer_address), amount: u256::new(1_000), asset: None, id: u256::ZERO, from_evm: false }, Some(gateway), None)).unwrap();
        let salt = crate::crypto::hasher::Hasher::digest(
            crate::structs::hash::HashType::Keccak256,
            &encode_values(&[AbiType::Uint(64), AbiType::Uint(8), AbiType::Bytes], &[AbiValue::Uint(u256::from(here)), AbiValue::Uint(u256::from(1u64)), AbiValue::Bytes(Vec::new())]).unwrap(),
        )
        .to_bytes();
        let wrapped_dev = || match evm_read(&evm_gateway, "wrappedOf(bytes32)", &[AbiValue::FixedBytes(salt.clone())], &[AbiType::Address])[..] {
            [AbiValue::Address(address)] => address,
            _ => unreachable!(),
        };
        wait("the coin reaching the other chain", &mut || {
            one.pass(0);
            two.pass(0);
            wrapped_dev() != [0; 20]
        });
        one.pass(0);
        two.pass(0);
        assert!(one.refused_now().is_empty(), "the key the relay asks by is Gateway.sol's: {:?}", one.refused_now());
        let dev = wrapped_dev();

        // The gateway set up anew, as after a reset; Gateway.sol's logs still name the last one, and
        // reading them neither stops a pass nor signs anything for the new one.
        let renewed = deployed_at(&me, nonce_of() + 3, hash);
        bridge(args(BridgeAction::Deploy, None, None)).unwrap();
        bridge(args(BridgeAction::Peer { evm_gateway: to_hex(&evm_gateway) }, Some(renewed), Some(relayer.clone()))).unwrap();
        let one = Bridge::new(args(BridgeAction::Relay, Some(renewed), Some(signer_one.clone()))).unwrap();
        let two = Bridge::new(args(BridgeAction::Relay, Some(renewed), Some(signer_two.clone()))).unwrap();
        one.pass(0);
        two.pass(0);
        assert!(one.refused_now().is_empty() && two.refused_now().is_empty(), "old logs are nothing refused: {:?}", one.refused_now());
        assert_eq!(read(&renewed, "received", serde_json::json!([{ "type": "u64", "value": rpc.chain_id().unwrap().to_string() }, { "type": "u64", "value": "1" }]))[0]["value"], false);
        assert!(wrapped_here(&renewed, &unreadable).is_none(), "not minted again");

        // Wrapped coin sent back: the new gateway holds none to release, so it refuses this at the
        // threshold, every time. The token sent after it arrives anyway.
        bridge(args(BridgeAction::Send { kind: "token".into(), recipient: bob.to_string(), amount: u256::new(100), asset: Some(to_hex(&dev)), id: u256::ZERO, from_evm: true }, Some(renewed), Some(relayer.clone()))).unwrap();
        let plain = deploy_token(b"Plain");
        send_token(plain, 25, renewed);
        // And what leaves here counts from 1 again, under this gateway's own key at Gateway.sol.
        bridge(args(BridgeAction::Send { kind: "coin".into(), recipient: to_hex(&relayer_address), amount: u256::new(7), asset: None, id: u256::ZERO, from_evm: false }, Some(renewed), None)).unwrap();
        let dev_balance = |holder: [u8; 20]| evm_read(&dev, "balanceOf(address)", &[AbiValue::Address(holder)], &[AbiType::Uint(256)]);
        let before = dev_balance(relayer_address);
        wait("the token after the refused transfer, and the relay", &mut || {
            one.pass(0);
            two.pass(0);
            wrapped_here(&renewed, &plain).is_some() && dev_balance(relayer_address) != before
        });
        assert_eq!(held_here(&wrapped_here(&renewed, &plain).unwrap(), &bob), "25");
        assert_eq!(dev_balance(relayer_address), [AbiValue::Uint(u256::from(1_000u64 - 100 + 7))], "the delivery, past what was refused");
        let refused = two.refused_now();
        assert!(refused.iter().any(|line| line.contains("signing transfer") && line.contains("would fail")), "listed, with its reason: {refused:?}");
        assert_eq!(read(&renewed, "received", serde_json::json!([{ "type": "u64", "value": rpc.chain_id().unwrap().to_string() }, { "type": "u64", "value": "2" }]))[0]["value"], false);

        a.stop();
        b.stop();
    }

    #[cfg(all(feature = "swap", feature = "sha256", feature = "ethereum"))]
    #[test]
    fn a_multisig_s_evm_contract_is_paid_from_synced_and_locks_a_swap_from_the_cli() {
        use crate::cli::multisig::evm::EvmAction;
        use crate::cli::multisig::{MultisigAction, MultisigArgs};
        use crate::cli::swap::rpc::{Rpc, from_key};
        use crate::cli::swap::{LegAction, SwapAction, SwapArgs, swap};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::structs::hex::to_hex;
        use crate::swap::{ChainLeg, EvmLeg, Leg, Parties, Terms, evm};

        let running = Running::start(true);
        let dir = TempDir::new("multisig-evm");
        let node = format!("http://{}", running.api);
        let eth_rpc = format!("{node}/rpc");
        let rpc = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &eth_rpc);

        // Three members, each a key here and an EVM key, whose files sign there and pay gas.
        let secrets: Vec<[u8; 32]> = (0x61..0x65).map(|seed| [seed; 32]).collect();
        let member = |at: usize| crate::cli::send::parse_member(&format!("{},,{}", Keypair::from_seed([0x70 + at as u8; 32]).public(), to_hex(&from_key(&secrets[at]).unwrap()))).unwrap();
        send(running.args(TxArgs::CreateMultisig { members: (0..3).map(member).collect(), threshold: 2 })).unwrap();
        let creator = DevAuthority::dev_keypair().public().address();
        let account = crate::structs::address::multisig_address(&creator, 0, crate::structs::hash::HashType::default_chain());
        let files: Vec<PathBuf> = secrets.iter().enumerate().map(|(at, secret)| {
            let path = dir.0.join(format!("evm{at}.key"));
            let keypair = Keypair::from_private(PrivateKey::Secp256k1(*secret));
            write_key(&path, &keypair).unwrap();
            send(running.args(TxArgs::Transfer { to: keypair.public().address().into(), amount: u256::from(10u64).pow(20) })).unwrap();

            path
        }).collect();
        let multisig = |file: &PathBuf, action: EvmAction| crate::cli::multisig(MultisigArgs { node: node.clone(), tls: Default::default(), key: Some(file.clone()), timeout: 10, action: MultisigAction::Evm { account, action } });
        let side = crate::cli::multisig::evm::evm_side(&crate::cli::send::agent(&Default::default()).unwrap(), &node, &account).unwrap();
        multisig(&files[0], EvmAction::Address).unwrap();

        // Coin sent to its address before it exists waits there.
        send(running.args(TxArgs::Transfer { to: Address::Evm(side.contract).into(), amount: u256::new(100_000) })).unwrap();
        assert!(!rpc.has_code(&side.contract).unwrap());

        // Member 0 proposes a payment; one signature is not enough; member 1 signs, and submitting
        // deploys the contract through the proxy, then pays.
        let pay = dir.0.join("pay.evm");
        let payee = [0x99; 20];
        multisig(&files[0], EvmAction::Propose { eth_rpc: eth_rpc.clone(), to: to_hex(&payee), value: "300".into(), data: None, nonce: None, out: pay.clone() }).unwrap();
        let short = multisig(&files[3], EvmAction::Submit { bundle: pay.clone(), eth_rpc: eth_rpc.clone() }).unwrap_err();
        assert!(short.to_string().contains("1 of the contract's owners signed"), "{short}");
        // It shows each call, and takes a typed yes.
        let read = fs::read_to_string(&pay).unwrap().lines().map(|line| crate::abi::multisig::EvmBundle::from_text(line).unwrap()).collect::<Vec<_>>();
        assert!(crate::cli::multisig::evm::review(&read, false, &mut "no\n".as_bytes()).unwrap_err().to_string().contains("nothing is signed"));
        crate::cli::multisig::evm::review(&read, false, &mut "yes\n".as_bytes()).unwrap();
        multisig(&files[1], EvmAction::Sign { bundle: pay.clone(), yes: true }).unwrap();
        multisig(&files[3], EvmAction::Submit { bundle: pay.clone(), eth_rpc: eth_rpc.clone() }).unwrap();

        assert!(rpc.has_code(&side.contract).unwrap());
        assert_eq!(rpc.balance(&payee).unwrap(), u256::new(300));
        assert_eq!(rpc.balance(&side.contract).unwrap(), u256::new(99_700), "what waited for it, less what it paid");
        multisig(&files[3], EvmAction::Status { eth_rpc: eth_rpc.clone() }).unwrap();
        // Sent again, it ran already.
        multisig(&files[3], EvmAction::Submit { bundle: pay.clone(), eth_rpc: eth_rpc.clone() }).unwrap();
        assert_eq!(rpc.balance(&payee).unwrap(), u256::new(300));

        // Member 2 leaves for a fourth here; sync proposes the same there, which the owners before
        // sign.
        let change = dir.0.join("change.bundle");
        let members: Vec<PathBuf> = (0..2).map(|at| {
            let path = dir.0.join(format!("member{at}.key"));
            write_key(&path, &Keypair::from_seed([0x70 + at as u8; 32])).unwrap();

            path
        }).collect();
        send(running.args(TxArgs::Transfer { to: account.into(), amount: u256::from(10u64).pow(18) })).unwrap();
        send(SendArgs {
            key: Some(members[0].clone()),
            multisig: Some(account),
            out: Some(change.clone()),
            ..running.args(TxArgs::ChangeMultisig { members: vec![member(0), member(1), member(3)], threshold: 2 })
        })
        .unwrap();
        crate::cli::multisig(MultisigArgs { node: node.clone(), tls: Default::default(), key: Some(members[1].clone()), timeout: 10, action: MultisigAction::Sign { bundle: change, submit: true, yes: true, full: false } }).unwrap();

        let sync = dir.0.join("sync.evm");
        multisig(&files[0], EvmAction::Sync { eth_rpc: eth_rpc.clone(), out: sync.clone() }).unwrap();
        multisig(&files[1], EvmAction::Sign { bundle: sync.clone(), yes: true }).unwrap();
        multisig(&files[3], EvmAction::Submit { bundle: sync, eth_rpc: eth_rpc.clone() }).unwrap();
        let [owners, threshold, nonce] = crate::abi::multisig::multisig_reads().map(|input| rpc.call(&side.contract, &input).unwrap());
        let (now, nonce) = crate::abi::multisig::multisig_read(&owners, &threshold, &nonce).unwrap();

        assert_eq!((Some(now), nonce), (crate::cli::multisig::evm::evm_side(&crate::cli::send::agent(&Default::default()).unwrap(), &node, &account).unwrap().now, u256::new(2)));
        let synced = dir.0.join("again.evm");
        multisig(&files[0], EvmAction::Sync { eth_rpc: eth_rpc.clone(), out: synced.clone() }).unwrap();
        assert!(!synced.exists(), "nothing to sync");

        // It pays 7,000 wei into a swap as the starter: the lock is its contract's call, a bundle
        // the new member signs too.
        let swap_sol = rpc.deploy(&evm::init_code(), &secrets[3]).unwrap();
        let (_, hash) = crate::swap::secret().unwrap();
        let starter = Parties { chain: Some(account), bitcoin: None, evm: Some(side.contract) };
        let bob = Parties { chain: Some(Address::Ed25519([0xb; 32])), bitcoin: None, evm: Some([0xb0; 20]) };
        let pays = Leg::Evm(EvmLeg { item: None, chain_id: rpc.chain_id().unwrap(), contract: swap_sol, token: None, amount: u256::new(7_000), payer: None, receiver: None });
        let gets = Leg::Chain(ChainLeg { item: None, contract: Address::Contract([0x5a; 32]), token: None, amount: u256::new(5_000), payer: None, receiver: None });
        let now = now_ms();
        let terms = Terms::offer(hash, pays, gets, now + crate::swap::FIRST_DEADLINE_MS, now + crate::swap::SECOND_DEADLINE_MS, &starter).unwrap().answer(&bob).unwrap();
        let lock = dir.0.join("lock.evm");
        let action = LegAction { terms: terms.to_text(), leg: None, node: node.clone(), eth_rpc: vec![eth_rpc.clone()], eth_auth: Default::default(), proxy: Default::default(), tls: Default::default(), key: Some(files[0].clone()), anyway: false, multisig: Some(account), out: Some(lock.clone()) };
        swap(SwapArgs { action: SwapAction::Lock(action) }).unwrap();
        multisig(&files[3], EvmAction::Sign { bundle: lock.clone(), yes: true }).unwrap();
        multisig(&files[3], EvmAction::Submit { bundle: lock, eth_rpc: eth_rpc.clone() }).unwrap();
        let locked = evm::read_lock(&rpc.call(&swap_sol, &evm::get_input(&terms.swap_id(crate::swap::Side::First).unwrap())).unwrap()).unwrap();
        running.stop();

        assert_eq!(terms.first.stands(&locked, terms.first_deadline), Ok(()));
    }

    /// Foundry's anvil, a private EVM chain with the deployment proxy, on its own port.
    #[cfg(all(feature = "swap", feature = "ethereum"))]
    struct Anvil(std::process::Child);

    #[cfg(all(feature = "swap", feature = "ethereum"))]
    impl Drop for Anvil {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(all(feature = "swap", feature = "sha256", feature = "ethereum"))]
    #[test]
    #[ignore = "runs Foundry's anvil on a private chain"]
    fn a_multisig_s_contract_sits_at_one_address_on_anvil_and_chainv4_and_pays_on_anvil() {
        use crate::cli::multisig::evm::EvmAction;
        use crate::cli::multisig::{MultisigAction, MultisigArgs};
        use crate::cli::swap::rpc::{Rpc, from_key};
        use crate::crypto::keypair::{Keypair, PrivateKey};
        use crate::structs::hex::to_hex;
        use crate::swap::evm::Call;

        let anvil = Anvil(std::process::Command::new("anvil").args(["--port", "18965", "--silent"]).spawn().expect("is Foundry's anvil installed?"));
        let anvil_rpc = "http://127.0.0.1:18965".to_string();
        let on_anvil = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &anvil_rpc);
        let started = std::time::Instant::now();
        while on_anvil.chain_id().is_err() {
            assert!(started.elapsed() < std::time::Duration::from_secs(20), "anvil did not start");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // anvil's first account, funded, from its well-known words.
        let rich = crate::structs::hex::from_hex("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80").unwrap().try_into().unwrap();

        let running = Running::start(true);
        let dir = TempDir::new("multisig-anvil");
        let node = format!("http://{}", running.api);
        let secrets: Vec<[u8; 32]> = (0x81..0x84).map(|seed| [seed; 32]).collect();
        let member = |at: usize| crate::cli::send::parse_member(&format!("{},,{}", Keypair::from_seed([0x90 + at as u8; 32]).public(), to_hex(&from_key(&secrets[at]).unwrap()))).unwrap();
        send(running.args(TxArgs::CreateMultisig { members: (0..3).map(member).collect(), threshold: 2 })).unwrap();
        let account = crate::structs::address::multisig_address(&DevAuthority::dev_keypair().public().address(), 0, crate::structs::hash::HashType::default_chain());
        let files: Vec<PathBuf> = secrets.iter().enumerate().map(|(at, secret)| {
            let path = dir.0.join(format!("evm{at}.key"));
            let keypair = Keypair::from_private(PrivateKey::Secp256k1(*secret));
            write_key(&path, &keypair).unwrap();
            send(running.args(TxArgs::Transfer { to: keypair.public().address().into(), amount: u256::from(10u64).pow(20) })).unwrap();
            on_anvil.send(&Call { to: from_key(secret).unwrap(), value: u256::from(10u64).pow(18), input: Vec::new(), what: "Fund a member" }, false, &rich).unwrap();

            path
        }).collect();
        let multisig = |action: EvmAction| crate::cli::multisig(MultisigArgs { node: node.clone(), tls: Default::default(), key: Some(files[0].clone()), timeout: 10, action: MultisigAction::Evm { account, action } });
        let side = crate::cli::multisig::evm::evm_side(&crate::cli::send::agent(&Default::default()).unwrap(), &node, &account).unwrap();

        // Ether sent to its address on anvil before it exists; a payment from it, signed by two.
        on_anvil.send(&Call { to: side.contract, value: u256::new(1_000_000), input: Vec::new(), what: "Fund the multisig" }, false, &rich).unwrap();
        let pay = dir.0.join("pay.evm");
        multisig(EvmAction::Propose { eth_rpc: anvil_rpc.clone(), to: to_hex(&[0x99; 20]), value: "400000".into(), data: None, nonce: None, out: pay.clone() }).unwrap();
        crate::cli::multisig(MultisigArgs { node: node.clone(), tls: Default::default(), key: Some(files[2].clone()), timeout: 10, action: MultisigAction::Evm { account, action: EvmAction::Sign { bundle: pay.clone(), yes: true } } }).unwrap();
        multisig(EvmAction::Submit { bundle: pay, eth_rpc: anvil_rpc.clone() }).unwrap();

        assert_eq!(on_anvil.balance(&[0x99; 20]).unwrap(), u256::new(400_000));
        assert_eq!(on_anvil.balance(&side.contract).unwrap(), u256::new(600_000));

        // On chainv4's own EVM it lands at the same address.
        let on_chainv4 = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &format!("{node}/rpc"));
        multisig(EvmAction::Deploy { eth_rpc: format!("{node}/rpc") }).unwrap();

        assert!(on_anvil.has_code(&side.contract).unwrap() && on_chainv4.has_code(&side.contract).unwrap());
        running.stop();
        drop(anvil);
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
        // the EVM chain this swap's second leg is on. The chain has the deployment proxy from
        // genesis, so Swap.sol lands where it does on every chain.
        let bob_key = Keypair::from_private(PrivateKey::Secp256k1([0x42; 32]));
        let bob_file = dir.0.join("bob.key");
        fs::write(&bob_file, bob_key.to_key_file()).unwrap();
        send(running.args(TxArgs::Transfer { to: bob_key.public().address().into(), amount: u256::from(10u64).pow(20) })).unwrap();
        let rpc = Rpc::new(crate::cli::send::agent(&Default::default()).unwrap(), &format!("{node}/rpc"));
        let swap_sol = rpc.deploy(&evm::init_code(), &[0x42; 32]).unwrap();
        assert_eq!(swap_sol, evm::deployed_address(), "through the proxy");
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
        let action = |key: Option<PathBuf>, leg: Option<LegArg>| LegAction { terms: terms.to_text(), leg, node: node.clone(), eth_rpc: vec![format!("{node}/rpc")], eth_auth: Default::default(), proxy: Default::default(), tls: Default::default(), key, anyway: false, multisig: None, out: None };

        // Bob cannot lock before Alice: her lock does not stand yet.
        let early = swap(SwapArgs { action: SwapAction::Lock(action(Some(bob_file.clone()), None)) }).unwrap_err();
        assert!(early.to_string().contains("nothing is locked yet"), "{early}");

        swap(SwapArgs { action: SwapAction::Lock(action(None, None)) }).unwrap();

        // Alice's lock stands, but nothing on this node is final: Bob is held, until he says so.
        let unfinal = swap(SwapArgs { action: SwapAction::Lock(action(Some(bob_file.clone()), None)) }).unwrap_err();
        assert!(unfinal.to_string().contains("not final yet") && unfinal.to_string().contains("--anyway"), "{unfinal}");
        swap(SwapArgs { action: SwapAction::Lock(LegAction { anyway: true, ..action(Some(bob_file.clone()), None) }) }).unwrap();
        let id = terms.swap_id(crate::swap::Side::Second).unwrap();
        let locked = evm::read_lock(&rpc.call(&swap_sol, &evm::get_input(&id)).unwrap()).unwrap();
        assert_eq!(terms.second.stands(&locked, terms.second_deadline), Ok(()));
        assert!(locked.height.is_some(), "Swap.sol says the block it was locked in");

        // Alice claims Bob's lock on the EVM, sent by anyone, which shows the secret there: held
        // too, as its block is not final, until she says so.
        let held = swap(SwapArgs { action: SwapAction::Claim { action: action(Some(bob_file.clone()), Some(LegArg::Second)), secret: to_hex(&secret) } }).unwrap_err();
        assert!(held.to_string().contains("not final yet"), "{held}");
        swap(SwapArgs { action: SwapAction::Claim { action: LegAction { anyway: true, ..action(Some(bob_file.clone()), Some(LegArg::Second)) }, secret: to_hex(&secret) } }).unwrap();
        let claimed = evm::read_lock(&rpc.call(&swap_sol, &evm::get_input(&id)).unwrap()).unwrap();
        assert_eq!((claimed.state, claimed.secret, claimed.owed), (LockState::Claimed, Some(secret), false));
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
