use {
    super::*,
    chainv4::consensus::{ConsensusConfig, PowConfig, Schedule},
    chainv4::crypto::keypair::{Keypair, PrivateKey},
    chainv4::params::BlockLimits,
    chainv4::params::Limits,
    chainv4::params::Reward,
    chainv4::params::fees::{BaseFee, Bid, ContractScorer, FeeConfig, PricingConfig, ScheduleConfig, Weighted},
    chainv4::params::finality::{AuthorityConfig, DepthConfig, FfgConfig, FinalityConfig, GenesisValidator},
    chainv4::params::json::change_json,
    chainv4::registry::directory::{Directory, EvmGateway},
    chainv4::registry::federation::Federation,
    chainv4::registry::validators::Leak,
    chainv4::structs::hash::HashType,
    chainv4::structs::u256::u256,
    chainv4::structs::version::{Upgrade, Version},
};

const DEV: u128 = 1_000_000_000_000_000_000;

fn env<'a>() -> Env<'a> {
    Env { decimals: 18, symbol: "DEV", epoch: 40, tip: Some(1_000), finality: Some("ffg"), consensus: Some("pow"), switching: false }
}

fn compressed(byte: u8) -> [u8; 33] {
    match Keypair::from_private(PrivateKey::Bitcoin([byte; 32])).public() {
        PublicKey::Bitcoin(bytes) => bytes,
        _ => unreachable!("a bitcoin key"),
    }
}

fn key(byte: u8) -> PublicKey {
    Keypair::from_seed([byte; 32]).public()
}

fn address(byte: u8) -> Address {
    Address::Ed25519([byte; 32])
}

fn ffg(leak: Option<Leak>) -> FinalityConfig {
    FinalityConfig::Ffg(FfgConfig {
        checkpoint_interval: 8,
        vote_depth: 2,
        min_bond: u256::new(100 * DEV),
        activation_delay: 2,
        withdrawal_delay: 64,
        reporter_divisor: 16,
        max_validators: 3,
        vote_reward: u256::new(DEV),
        max_proofs_per_block: 4,
        validators: vec![GenesisValidator { account: address(1), voter: key(1), bond: u256::new(10_000 * DEV) }],
        leak,
        slash_permille: None,
    })
}

fn authority() -> FinalityConfig {
    FinalityConfig::Authority(AuthorityConfig {
        checkpoint_interval: 8,
        vote_depth: 2,
        activation_delay: 2,
        max_signers: 10,
        signers: vec![Signer { key: key(1), account: address(1) }, Signer { key: key(2), account: address(2) }, Signer { key: key(3), account: address(3) }],
        threshold: 2,
        change_threshold: 2,
        approval_epochs: 64,
        vote_reward: u256::ZERO,
        max_proofs_per_block: 4,
    })
}

fn fees(schedule: ScheduleConfig, pricing: PricingConfig) -> FeeConfig {
    FeeConfig { block_gas_limit: 50_000_000, burn_permille: 500, schedule, pricing, costs: None, vote_rewards: None }
}

fn weights() -> Weighted {
    Weighted { per_tx: 1_000, per_byte: 10, per_code_byte: 20, fuel_per_gas: 1 }
}

fn base_fee() -> PricingConfig {
    PricingConfig::BaseFee(BaseFee { initial: u256::new(1_000_000_000), min: u256::new(10_000_000), elasticity: 2, change_denominator: 8 })
}

fn directory() -> Directory {
    Directory {
        gateway: Some(Address::Contract([1; 32])),
        btc_gateway: Some(Address::Contract([2; 32])),
        evm_gateways: vec![EvmGateway { chain_id: 1, gateway: [3; 20] }, EvmGateway { chain_id: 5, gateway: [4; 20] }],
        swap: Some(Address::Contract([5; 32])),
        dex: Some(Address::Contract([6; 32])),
        dex_router: Some([7; 20]),
        nft_factory: Some(Address::Contract([8; 32])),
    }
}

/// One of every shape every kind's change takes.
fn every_shape() -> Vec<Change> {
    vec![
        Change::Name("chainv4 mainnet".to_string()),
        Change::Bootnodes(vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string()]),
        Change::Names(chainv4::registry::names::NameSettings { prices: vec![u256::new(5 * DEV), u256::new(DEV)], grace_ms: 86_400_000, max_years: 50 }),
        Change::TokenFunctions(vec!["balance_of".to_string(), "transfer".to_string()]),
        Change::Governance(chainv4::governance::GovernanceConfig::Council { members: vec![Address::Ed25519([1; 32]), Address::Ed25519([2; 32])], threshold: 2, approval_epochs: 64, quorum_permille: 500 }),
        Change::EvmSpec { spec: chainv4::params::vm::EvmSpec::Osaka, from: 5_000 },
        Change::Dex(chainv4::params::vm::DexSettings { fee_bps: 25 }),
        Change::Anchoring(chainv4::params::anchoring::AnchorSettings::OFF),
        Change::Anchoring(chainv4::params::anchoring::AnchorSettings {
            bitcoin: true,
            ethereum: true,
            enforce: true,
            relay: Some(Address::Contract([4; 32])),
            btc_start: Some(chainv4::params::anchoring::BtcStart { header: vec![7; 80], height: 4032, network: chainv4::params::anchoring::BtcNetworkRules::Main }),
            eth_relay: Some(Address::Contract([5; 32])),
            eth_start: Some(chainv4::params::anchoring::EthStart { root: [6; 32], network: chainv4::params::anchoring::EthNetwork::Preset("sepolia".into()) }),
            depth: 3,
            every: 45,
            reward: u256::new(2 * DEV),
        }),
        Change::Anchoring(chainv4::params::anchoring::AnchorSettings {
            ethereum: true,
            eth_relay: Some(Address::Contract([5; 32])),
            eth_start: Some(chainv4::params::anchoring::EthStart { root: [6; 32], network: chainv4::params::anchoring::EthNetwork::Custom(ethlight_network()) }),
            ..chainv4::params::anchoring::AnchorSettings::OFF
        }),
        Change::Upgrade(Upgrade { least: Version::new(0, 2, 0), from: 2_000 }),
        Change::Consensus(ConsensusConfig::ProofOfWork(PowConfig { hash: HashType::Blake3_256, target_block_ms: 2_000, initial_difficulty: 100_000, min_difficulty: 1_000, tuning: None, window: None })),
        Change::Consensus(ConsensusConfig::Authority { key: key(1), slot_ms: 1_000 }),
        Change::Consensus(ConsensusConfig::Rotating { slot_ms: 2_000 }),
        Change::Consensus(ConsensusConfig::Staked { slot_ms: 2_000, schedule: Schedule::Priority }),
        Change::Fees(fees(ScheduleConfig::Weighted(weights()), base_fee())),
        Change::Fees(FeeConfig { vote_rewards: Some(chainv4::params::fees::VoteRewardSource::Fees), ..fees(ScheduleConfig::Weighted(weights()), base_fee()) }),
        Change::Fees(fees(ScheduleConfig::Contract(ContractScorer { weights: weights(), contract: Address::Contract([9; 32]), fuel: 1_000, kinds: vec!["call".to_string()] }), PricingConfig::Bid(Bid { min_price: u256::new(7) }))),
        Change::Reward(Reward { initial: u256::new(50 * DEV), halving_interval: Some(210_000) }),
        Change::Limits(BlockLimits { tx_limits: Limits { fuel: 10_000_000, memory: 32 << 20 }, max_block_bytes: Some(4 << 20), contract: None, max_tx_bytes: None, multisig_members: None }),
        Change::Finality(ffg(Some(Leak { after_epochs: 4, permille: 10 }))),
        Change::Finality(authority()),
        Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 6 })),
        Change::SwitchFinality { epoch: 44, config: authority() },
        Change::SwitchFinality { epoch: 44, config: match ffg(Some(Leak { after_epochs: 4, permille: 10 })) {
            FinalityConfig::Ffg(config) => FinalityConfig::Ffg(FfgConfig { validators: Vec::new(), ..config }),
            other => other,
        } },
        Change::Collections(chainv4::registry::collections::CollectionFunctions::standard()),
        Change::Federation(Federation { signers: vec![compressed(2), compressed(3), compressed(4)], threshold: 2, chain_depth: 20, evm_depth: 12, bitcoin_depth: 6, btc: None }),
        Change::Directory(directory()),
    ]
}

/// The paths of a JSON value's leaves: an empty object or list, or anything else.
fn leaves(value: &Value, path: String, found: &mut Vec<String>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                let next = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                leaves(value, next, found);
            }
        }
        _ => found.push(path),
    }
}

#[test]
fn every_field_of_every_kind_of_change_has_a_place_in_its_form() {
    for change in every_shape() {
        let kind = change.name();
        let value = change_json(&change)[kind].clone();
        let specs: Vec<Spec> = schema(kind).into_iter().filter(|spec| shows_for(spec, &value)).collect();
        let mut found = Vec::new();
        leaves(&value, String::new(), &mut found);

        for leaf in found {
            // A switch into FFG names no validators, and says so by leaving the field out.
            if kind == "switch_finality" && leaf == "finality.validators" {
                continue;
            }

            assert!(specs.iter().any(|spec| spec.path == leaf), "{kind}: no field for {leaf}");
        }
    }

    // Every kind the page lists has fields.
    for kind in crate::views::governance::KINDS {
        assert!(!schema(kind).is_empty(), "{kind} has no form");
    }
}

#[test]
fn a_change_read_into_the_form_builds_itself_back_and_moves_nothing() {
    let env = env();

    for change in every_shape() {
        let kind = change.name();

        // A switch's form starts from the other kind: that one has a test of its own.
        if kind == "switch_finality" {
            continue;
        }

        let value = change_json(&change)[kind].clone();
        let env = Env { finality: Some(if matches!(change, Change::Consensus(ConsensusConfig::Rotating { .. })) { "authority" } else { "ffg" }), ..env };
        let draft = Draft::of(kind, Some(&value), &env);
        let built = build(kind, &draft, Some(&value), &env);

        assert_eq!(built.errors, BTreeMap::new(), "{kind}");
        assert_eq!(built.change.as_ref(), Some(&change), "{kind}");
        assert!(built.changes.is_empty(), "{kind} moved {:?}", built.changes);
    }
}

#[test]
fn only_what_changed_is_given_and_laid_over_what_stands() {
    let env = env();
    let stands = change_json(&Change::Fees(fees(ScheduleConfig::Weighted(weights()), base_fee())))["fees"].clone();
    let mut draft = Draft::of("fees", Some(&stands), &env);

    draft.texts.insert("burn_permille".to_string(), "600".to_string());
    let built = build("fees", &draft, Some(&stands), &env);

    assert_eq!(built.given, json!({ "burn_permille": 600 }));
    assert_eq!(built.change, Some(Change::Fees(FeeConfig { burn_permille: 600, ..fees(ScheduleConfig::Weighted(weights()), base_fee()) })));
    assert_eq!(built.changes, [Diff { label: "Share of each fee burned, in thousandths".to_string(), old: "50%".to_string(), new: "60%".to_string() }]);

    // Two levels down, the same.
    draft.texts.insert("pricing.min".to_string(), "0.0000000001".to_string());
    let built = build("fees", &draft, Some(&stands), &env);

    assert_eq!(built.given, json!({ "burn_permille": 600, "pricing": { "min": "100000000" } }));
    assert_eq!(built.changes[1], Diff { label: "Least price".to_string(), old: "0.00000000001 DEV".to_string(), new: "0.0000000001 DEV".to_string() });

    // A least price past the one it starts from: the node's check, under its field.
    draft.texts.insert("pricing.min".to_string(), "0.1".to_string());
    let built = build("fees", &draft, Some(&stands), &env);

    assert_eq!(built.change, None);
    assert_eq!(built.errors.get("pricing.initial").map(String::as_str), Some("the base fee's initial price must be at least its min"), "the node's check, under its field");
}

#[test]
fn where_vote_rewards_come_from_is_picked_or_left_as_genesis_set() {
    use chainv4::params::fees::VoteRewardSource;

    let env = env();
    let base = fees(ScheduleConfig::Weighted(weights()), base_fee());
    let stands = change_json(&Change::Fees(base.clone()))["fees"].clone();
    let mut draft = Draft::of("fees", Some(&stands), &env);

    // Left as genesis set: nothing changes.
    assert_eq!(draft.text("vote_rewards"), "");
    assert_eq!(build("fees", &draft, Some(&stands), &env).given, json!({}));

    // Paid from the reward pool: that alone is given, and said in words.
    draft.texts.insert("vote_rewards".to_string(), "fees".to_string());
    let built = build("fees", &draft, Some(&stands), &env);
    assert_eq!(built.given, json!({ "vote_rewards": "fees" }));
    assert_eq!(built.change, Some(Change::Fees(FeeConfig { vote_rewards: Some(VoteRewardSource::Fees), ..base.clone() })));
    assert_eq!(built.changes, [Diff { label: "Where vote rewards come from".to_string(), old: "As genesis".to_string(), new: "Reward pool".to_string() }]);

    // Back from minting to as genesis set, over one that names it.
    let minting = change_json(&Change::Fees(FeeConfig { vote_rewards: Some(VoteRewardSource::Mint), ..base.clone() }))["fees"].clone();
    let mut draft = Draft::of("fees", Some(&minting), &env);
    assert_eq!(draft.text("vote_rewards"), "mint");
    draft.texts.insert("vote_rewards".to_string(), String::new());
    let built = build("fees", &draft, Some(&minting), &env);
    assert_eq!(built.given, json!({ "vote_rewards": null }));
    assert_eq!(built.change, Some(Change::Fees(base)));

    // Anything else is refused at its field.
    draft.texts.insert("vote_rewards".to_string(), "airdrop".to_string());
    assert!(build("fees", &draft, Some(&minting), &env).errors.contains_key("vote_rewards"));
}

#[test]
fn another_variant_replaces_its_object_whole() {
    let env = env();
    let stands = change_json(&Change::Fees(fees(ScheduleConfig::Weighted(weights()), base_fee())))["fees"].clone();
    let mut draft = Draft::of("fees", Some(&stands), &env);

    draft.texts.insert("pricing.type".to_string(), "bid".to_string());
    draft.texts.insert("pricing.min_price".to_string(), "0.000000000000000007".to_string());
    let built = build("fees", &draft, Some(&stands), &env);

    assert_eq!(built.errors, BTreeMap::new());
    assert_eq!(built.given, json!({ "pricing": { "type": "bid", "min_price": "7" } }));
    assert_eq!(built.change, Some(Change::Fees(fees(ScheduleConfig::Weighted(weights()), PricingConfig::Bid(Bid { min_price: u256::new(7) })))));
    assert_eq!(built.changes[0], Diff { label: "How gas is priced".to_string(), old: "A base price".to_string(), new: "Bids".to_string() });

    // A contract's fields come with it, and its kinds are checked.
    draft.texts.insert("schedule.type".to_string(), "contract".to_string());
    draft.texts.insert("schedule.contract".to_string(), Address::Contract([9; 32]).to_string());
    draft.lists.insert("schedule.kinds".to_string(), vec!["call".to_string(), "weather".to_string()]);
    let built = build("fees", &draft, Some(&stands), &env);

    assert!(built.errors["schedule.kinds#1"].starts_with("Not a kind of transaction"), "{:?}", built.errors);

    draft.lists.insert("schedule.kinds".to_string(), vec!["call".to_string()]);
    let built = build("fees", &draft, Some(&stands), &env);

    match built.change {
        Some(Change::Fees(FeeConfig { schedule: ScheduleConfig::Contract(scorer), .. })) => {
            assert_eq!((scorer.contract, scorer.kinds), (Address::Contract([9; 32]), vec!["call".to_string()]));
        }
        other => panic!("{other:?} {:?}", built.errors),
    }

    // A consensus of another kind is another object.
    let pow = json!({ "type": "pow", "hash": "blake3", "target_block_ms": 2000, "initial_difficulty": "100000", "min_difficulty": "1000" });
    let mut draft = Draft::of("consensus", Some(&pow), &env);
    draft.texts.insert("type".to_string(), "authority".to_string());
    draft.texts.insert("key".to_string(), key(1).to_string());
    let built = build("consensus", &draft, Some(&pow), &env);

    assert_eq!(built.change, Some(Change::Consensus(ConsensusConfig::Authority { key: key(1), slot_ms: 2_000 })));
    assert_eq!(built.changes.iter().map(|change| change.label.as_str()).collect::<Vec<_>>(), ["Who makes blocks", "The key that makes blocks", "Time between blocks, in ms"]);
}

#[test]
fn numbers_and_amounts_read_as_they_are_typed() {
    let env = env();
    let stands = json!({ "initial": "50000000000000000000", "halving_interval": 210000 });
    let mut draft = Draft::of("reward", Some(&stands), &env);

    assert_eq!(draft.text("initial"), "50", "in the coin");
    assert_eq!(draft.text("halving_interval"), "210000");

    draft.texts.insert("initial".to_string(), "1,000.5".to_string());
    draft.texts.insert("halving_interval".to_string(), "420_000".to_string());
    let built = build("reward", &draft, Some(&stands), &env);
    assert_eq!(built.change, Some(Change::Reward(Reward { initial: u256::new(1_000 * DEV + DEV / 2), halving_interval: Some(420_000) })));
    assert_eq!(built.changes[0].new, "1,000.5 DEV");

    draft.switches.insert("halving_interval".to_string(), false);
    let built = build("reward", &draft, Some(&stands), &env);
    assert_eq!(built.change, Some(Change::Reward(Reward { initial: u256::new(1_000 * DEV + DEV / 2), halving_interval: None })));
    assert_eq!(built.changes[1], Diff { label: "Halving".to_string(), old: "210,000".to_string(), new: "Never".to_string() });

    // Too many decimals, not a number, and 0 where the node asks for more.
    draft.texts.insert("initial".to_string(), "1.0000000000000000001".to_string());
    draft.switches.insert("halving_interval".to_string(), true);
    draft.texts.insert("halving_interval".to_string(), "0".to_string());
    let built = build("reward", &draft, Some(&stands), &env);
    assert!(built.errors["initial"].contains("at most 18 decimals"), "{:?}", built.errors);
    assert_eq!(built.errors["halving_interval"], "At least 1.");
    assert_eq!(built.change, None);

    // Bytes, in units or not.
    let limits = json!({ "tx_limits": { "fuel": 10_000_000, "memory": 33_554_432 }, "max_block_bytes": null });
    let mut draft = Draft::of("limits", Some(&limits), &env);
    assert_eq!(draft.text("tx_limits.memory"), "32 MiB");
    assert!(!draft.switches["max_block_bytes"]);

    draft.texts.insert("tx_limits.memory".to_string(), "64mib".to_string());
    draft.switches.insert("max_block_bytes".to_string(), true);
    draft.texts.insert("max_block_bytes".to_string(), "2097152".to_string());
    let built = build("limits", &draft, Some(&limits), &env);
    assert_eq!(built.change, Some(Change::Limits(BlockLimits { tx_limits: Limits { fuel: 10_000_000, memory: 64 << 20 }, max_block_bytes: Some(2 << 20), contract: None, max_tx_bytes: None, multisig_members: None })));
    assert_eq!(built.changes[0].new, "64 MiB");
    assert_eq!(built.changes[1], Diff { label: "Size of a block".to_string(), old: "Gas alone".to_string(), new: "2 MiB".to_string() });

    draft.texts.insert("max_block_bytes".to_string(), "7 MiB".to_string());
    draft.texts.insert("tx_limits.fuel".to_string(), "99999".to_string());
    let built = build("limits", &draft, Some(&limits), &env);
    assert_eq!(built.errors["max_block_bytes"], "At most 6 MiB.");
    assert_eq!(built.errors["tx_limits.fuel"], "At least 100,000.");
}

#[test]
fn what_the_form_refuses_at_a_field_the_node_refuses_too() {
    // Each of the bounds schema.rs gives a field, past its edge, as change_from_json reads it.
    let refused = [
        ("reward", json!({ "initial": "1", "halving_interval": 0 })),
        ("limits", json!({ "tx_limits": { "fuel": 99_999, "memory": 1 << 20 }, "max_block_bytes": null })),
        ("limits", json!({ "tx_limits": { "fuel": 100_000, "memory": (1 << 20) - 1 }, "max_block_bytes": null })),
        ("limits", json!({ "tx_limits": { "fuel": 100_000, "memory": 1 << 20 }, "max_block_bytes": (6 << 20) + 1 })),
        ("limits", json!({ "tx_limits": { "fuel": 100_000, "memory": 1 << 20 }, "max_block_bytes": (1 << 20) - 1 })),
    ];

    for (kind, value) in refused {
        assert!(change_from_json(&json!({ kind: value })).is_err(), "{kind} {value}");
    }

    let base = change_json(&Change::Fees(fees(ScheduleConfig::Weighted(weights()), base_fee())))["fees"].clone();

    for edit in [json!({ "burn_permille": 1001 }), json!({ "block_gas_limit": 0 }), json!({ "schedule": { "fuel_per_gas": 0 } }), json!({ "pricing": { "elasticity": 0 } }), json!({ "pricing": { "change_denominator": 0 } })] {
        assert!(change_from_json(&json!({ "fees": over(&base, &edit) })).is_err(), "{edit}");
    }

    let federation = change_json(&every_shape().into_iter().find(|change| change.name() == "federation").unwrap())["federation"].clone();

    for edit in [json!({ "threshold": 0 }), json!({ "chain_depth": 0 }), json!({ "evm_depth": 0 }), json!({ "bitcoin_depth": 0 })] {
        assert!(change_from_json(&json!({ "federation": over(&federation, &edit) })).is_err(), "{edit}");
    }
}

#[test]
fn the_node_s_errors_say_which_field_they_are_about() {
    let env = env();
    let under = |kind: &str, stands: &Value, edit: &dyn Fn(&mut Draft)| {
        let mut draft = Draft::of(kind, Some(stands), &env);
        edit(&mut draft);

        build(kind, &draft, Some(stands), &env).errors
    };

    // A federation's threshold past its signers, as its problem says.
    let federation = change_json(&every_shape().into_iter().find(|change| change.name() == "federation").unwrap())["federation"].clone();
    let errors = under("federation", &federation, &|draft| {
        draft.texts.insert("threshold".to_string(), "4".to_string());
    });
    assert!(errors["threshold"].contains("threshold"), "{errors:?}");

    // A signer that is no key on the curve: well formed, and only the node's check says.
    let errors = under("federation", &federation, &|draft| {
        draft.lists.get_mut("signers").unwrap()[0] = format!("0x05{}", "11".repeat(32));
    });
    assert!(errors["signers"].contains("compressed secp256k1"), "{errors:?}");

    // More than the most.
    let errors = under("bootnodes", &json!([]), &|draft| {
        draft.lists.insert(String::new(), (0..33).map(|port| format!("/ip4/203.0.113.7/udp/{port}/quic-v1")).collect());
    });
    assert_eq!(errors[""], "bootnodes: bootnodes holds at most 32 addresses");

    // The chain's name, whatever its length.
    let errors = under("name", &json!("second"), &|draft| {
        draft.texts.insert(String::new(), "x".repeat(41));
    });
    assert_eq!(errors[""], "name: a chain's name is 1 to 40 characters");

    assert_eq!(blame("fees", "fees.schedule.contract: cannot read x"), "schedule.contract");
    assert_eq!(blame("switch_finality", "finality.min_bond: x is not a whole amount"), "finality.min_bond");
    assert_eq!(blame("fees", "something else"), GENERAL);
    assert_eq!(blame("upgrade", "upgrade.least: not a version"), "least");
}

#[test]
fn what_is_wrong_goes_under_its_field_in_words() {
    let env = env();
    let check = |kind: &str, stands: Value, path: &str, typed: &str, says: &str| {
        let mut draft = Draft::of(kind, Some(&stands), &env);
        draft.texts.insert(path.to_string(), typed.to_string());
        let built = build(kind, &draft, Some(&stands), &env);

        assert!(built.errors.get(path).is_some_and(|why| why.contains(says)), "{kind}.{path}: {:?}", built.errors);
        assert_eq!(built.change, None);
    };

    check("name", json!("second"), "", "  ", "1 to 40");
    check("upgrade", json!({ "least": "0.1.0", "from": 5000 }), "least", "two", "A version");
    check("upgrade", json!({ "least": "0.1.0", "from": 5000 }), "from", "soon", "A whole number");
    check("upgrade", json!({ "least": "0.1.0", "from": 5000 }), "from", "1000", "ahead of the tip, block 1,000");
    check("upgrade", json!({ "least": "0.1.0", "from": 5000 }), "least", "0.0.0", "Above 0.0.0");
    check("directory", change_json(&Change::Directory(directory()))["directory"].clone(), "dex", "ed25519:0x01", "A contract's address");
    check("directory", change_json(&Change::Directory(directory()))["directory"].clone(), "dex_router", "0x1234", "An EVM address");
    check("directory", change_json(&Change::Directory(directory()))["directory"].clone(), "dex_router", &format!("0x{}", "00".repeat(20)), "Not the zero");
    check("fees", change_json(&Change::Fees(fees(ScheduleConfig::Weighted(weights()), base_fee())))["fees"].clone(), "burn_permille", "1001", "At most 1,000");
    check("consensus", json!({ "type": "authority", "key": key(1).to_string(), "slot_ms": 1000 }), "key", "nobody", "A key");

    // Lines of a list, by their place; empty ones are no lines.
    let stands = json!(["/ip4/203.0.113.7/udp/4001/quic-v1"]);
    let mut draft = Draft::of("bootnodes", Some(&stands), &env);
    draft.lists.get_mut("").unwrap().extend(["".to_string(), "not an address".to_string(), "/ip4/203.0.113.7/udp/4001/quic-v1".to_string()]);
    let built = build("bootnodes", &draft, Some(&stands), &env);
    assert!(built.errors["#2"].starts_with("A libp2p address"), "{:?}", built.errors);
    assert_eq!(built.errors["#3"], "Named twice.");

    // A table's cell.
    let stands = change_json(&Change::Directory(directory()))["directory"].clone();
    let mut draft = Draft::of("directory", Some(&stands), &env);
    draft.tables.get_mut("evm_gateways").unwrap().push(vec!["0".to_string(), "0x12".to_string()]);
    let built = build("directory", &draft, Some(&stands), &env);
    assert_eq!(built.errors["evm_gateways#2.chain_id"], "A whole number above 0.");
    assert!(built.errors["evm_gateways#2.gateway"].starts_with("An EVM address"));
}

#[test]
fn lists_and_tables_add_and_remove_and_say_what_moved() {
    let env = env();
    let stands = json!(["/ip4/203.0.113.7/udp/4001/quic-v1", "/ip4/198.51.100.2/udp/4001/quic-v1"]);
    let mut draft = Draft::of("bootnodes", Some(&stands), &env);
    let lines = draft.lists.get_mut("").unwrap();
    lines.remove(0);
    lines.push("/dns4/boot.example/udp/4001/quic-v1".to_string());
    let built = build("bootnodes", &draft, Some(&stands), &env);

    assert_eq!(built.change, Some(Change::Bootnodes(vec!["/ip4/198.51.100.2/udp/4001/quic-v1".to_string(), "/dns4/boot.example/udp/4001/quic-v1".to_string()])));
    assert_eq!(
        built.changes,
        [
            Diff { label: "Bootnodes".to_string(), old: "/ip4/203.0.113.7/udp/4001/quic-v1".to_string(), new: "taken away".to_string() },
            Diff { label: "Bootnodes".to_string(), old: "not listed".to_string(), new: "/dns4/boot.example/udp/4001/quic-v1".to_string() },
        ]
    );

    let mut draft = Draft::of("bootnodes", Some(&stands), &env);
    draft.lists.get_mut("").unwrap().clear();
    assert_eq!(build("bootnodes", &draft, Some(&stands), &env).change, Some(Change::Bootnodes(Vec::new())), "none leaves the chain without");

    // Prices go by their place, the last for every longer name.
    let stands = json!({ "prices": ["640000000000000000000", "160000000000000000000", "5000000000000000000"], "grace_ms": 7_776_000_000u64, "max_years": 100 });
    let mut draft = Draft::of("names", Some(&stands), &env);
    assert_eq!(draft.lists["prices"], ["640", "160", "5"]);
    draft.lists.get_mut("prices").unwrap()[2] = "2.5".to_string();
    draft.lists.get_mut("prices").unwrap().push("1".to_string());
    let built = build("names", &draft, Some(&stands), &env);
    assert_eq!(built.change, Some(Change::Names(chainv4::registry::names::NameSettings { prices: vec![u256::new(640 * DEV), u256::new(160 * DEV), u256::new(5 * DEV / 2), u256::new(DEV)], grace_ms: 7_776_000_000, max_years: 100 })));
    assert_eq!(built.changes[0], Diff { label: "5 characters".to_string(), old: "5 DEV".to_string(), new: "2.5 DEV".to_string() });
    assert_eq!(built.changes[1], Diff { label: "6 or more characters".to_string(), old: "none".to_string(), new: "1 DEV".to_string() });

    let mut draft = Draft::of("names", Some(&stands), &env);
    draft.lists.get_mut("prices").unwrap().clear();
    assert!(build("names", &draft, Some(&stands), &env).errors["prices"].contains("at least one price"), "a price for every length");

    // A directory's chains go in order, whatever order they are typed in.
    let stands = change_json(&Change::Directory(directory()))["directory"].clone();
    let mut draft = Draft::of("directory", Some(&stands), &env);
    draft.tables.get_mut("evm_gateways").unwrap().insert(0, vec!["9".to_string(), format!("0x{}", "09".repeat(20))]);
    draft.texts.insert("dex".to_string(), String::new());
    let built = build("directory", &draft, Some(&stands), &env);

    match built.change {
        Some(Change::Directory(found)) => {
            assert_eq!(found.evm_gateways.iter().map(|gateway| gateway.chain_id).collect::<Vec<_>>(), [1, 5, 9]);
            assert_eq!(found.dex, None, "empty for none");
            assert_eq!(found.swap, directory().swap, "the rest stand");
        }
        other => panic!("{other:?} {:?}", built.errors),
    }
    assert_eq!(built.changes.iter().map(|change| change.label.as_str()).collect::<Vec<_>>(), ["Gateway.sol on EVM chains", "The DEX"]);
}

#[test]
fn a_kind_with_nothing_standing_starts_from_defaults_and_names_all_it_sets() {
    let env = env();
    let mut draft = Draft::of("directory", None, &env);
    let built = build("directory", &draft, None, &env);

    assert_eq!(built.change, Some(Change::Directory(Directory { gateway: None, btc_gateway: None, evm_gateways: Vec::new(), swap: None, dex: None, dex_router: None, nft_factory: None })));
    assert!(built.changes.is_empty(), "nothing yet: nothing to review");

    draft.texts.insert("dex".to_string(), Address::Contract([6; 32]).to_string());
    let built = build("directory", &draft, None, &env);
    assert_eq!(built.changes, [Diff { label: "The DEX".to_string(), old: "none".to_string(), new: Address::Contract([6; 32]).to_string() }]);

    // Fees on a chain without: all of them are what it would set.
    let draft = Draft::of("fees", None, &env);
    let built = build("fees", &draft, None, &env);
    assert!(built.change.is_some(), "{:?}", built.errors);
    assert!(built.changes.len() > 8, "{:?}", built.changes);

    // An upgrade is asked for.
    let built = build("upgrade", &Draft::of("upgrade", None, &env), None, &env);
    assert!(built.errors.contains_key("least") && built.errors.contains_key("from"), "{:?}", built.errors);

    // Nothing standing to put back to, and the standard collections a button away.
    let built = build("collections", &Draft::of("collections", None, &env), None, &env);
    assert!(built.changes.is_empty());
}

#[test]
fn a_retune_of_finality_keeps_what_the_chain_keeps() {
    let env = env();
    let stands = change_json(&Change::Finality(ffg(None)))["finality"].clone();
    let mut draft = Draft::of("finality", Some(&stands), &env);

    assert_eq!(draft.text("min_bond"), "100");
    assert!(schema("finality").iter().filter(|spec| spec.fixed).all(|spec| ["type", "checkpoint_interval", "activation_delay", "validators", "signers", "threshold", "change_threshold"].contains(&spec.path)), "fixed: what retune_problem keeps");

    draft.texts.insert("min_bond".to_string(), "500".to_string());
    draft.switches.insert("leak".to_string(), true);
    draft.texts.insert("leak.after_epochs".to_string(), "16".to_string());
    draft.texts.insert("leak.permille".to_string(), "10".to_string());
    let built = build("finality", &draft, Some(&stands), &env);

    match &built.change {
        Some(Change::Finality(FinalityConfig::Ffg(config))) => {
            assert_eq!(config.min_bond, u256::new(500 * DEV));
            assert_eq!(config.leak, Some(Leak { after_epochs: 16, permille: 10 }));
            assert_eq!(config.validators.len(), 1, "the validators from genesis stay");
            assert_eq!(config.checkpoint_interval, 8);
        }
        other => panic!("{other:?} {:?}", built.errors),
    }
    assert_eq!(built.given["leak"], json!({ "after_epochs": 16, "permille": 10 }));
    assert_eq!(
        built.changes.iter().map(|change| (change.label.as_str(), change.new.as_str())).collect::<Vec<_>>(),
        [("Smallest bond", "500 DEV"), ("Inactivity leak", "A leak"), ("After this many epochs without a proof", "16"), ("Thousandths of a bond each checkpoint", "1%")]
    );

    // Off again: the leak goes.
    let stands = change_json(&Change::Finality(ffg(Some(Leak { after_epochs: 4, permille: 10 }))))["finality"].clone();
    let mut draft = Draft::of("finality", Some(&stands), &env);
    draft.switches.insert("leak".to_string(), false);
    let built = build("finality", &draft, Some(&stands), &env);
    assert!(matches!(&built.change, Some(Change::Finality(FinalityConfig::Ffg(config))) if config.leak.is_none()), "{:?}", built.errors);
    assert_eq!(built.changes, [Diff { label: "Inactivity leak".to_string(), old: "A leak".to_string(), new: "None".to_string() }]);

    // Fewer validators than genesis named, as the node's check says.
    let stands = change_json(&Change::Finality(ffg(None)))["finality"].clone();
    let mut draft = Draft::of("finality", Some(&stands), &env);
    draft.texts.insert("max_validators".to_string(), "0".to_string());
    assert!(build("finality", &draft, Some(&stands), &env).errors["max_validators"].contains("Fewer than"));
}

#[test]
fn a_switch_goes_to_the_other_kind_no_sooner_than_the_chain_allows() {
    let env = env();
    let stands = json!({ "epoch": null, "finality": change_json(&Change::Finality(ffg(None)))["finality"] });
    let mut draft = Draft::of("switch_finality", Some(&stands), &env);

    assert_eq!(draft.text("finality.type"), "authority");
    assert_eq!(draft.text("epoch"), "43", "epoch 40, a delay of 2, and 1");
    assert_eq!(draft.text("finality.checkpoint_interval"), "8");
    assert_eq!(draft.text("finality.vote_depth"), "2", "what both kinds have starts from what stands");

    draft.tables.get_mut("finality.signers").unwrap().extend([
        vec![key(1).to_string(), address(1).to_string()],
        vec![key(2).to_string(), address(2).to_string()],
        vec![key(3).to_string(), address(3).to_string()],
    ]);
    draft.texts.insert("finality.threshold".to_string(), "2".to_string());
    draft.texts.insert("finality.change_threshold".to_string(), "2".to_string());
    let built = build("switch_finality", &draft, Some(&stands), &env);

    match &built.change {
        Some(Change::SwitchFinality { epoch, config: FinalityConfig::Authority(config) }) => {
            assert_eq!(*epoch, 43);
            assert_eq!((config.signers.len(), config.threshold, config.checkpoint_interval, config.activation_delay), (3, 2, 8, 2));
        }
        other => panic!("{other:?} {:?}", built.errors),
    }
    assert_eq!(built.changes[1], Diff { label: "Switch to".to_string(), old: "FFG, by stake".to_string(), new: "Authority, by signers".to_string() });

    // Too soon, and too few for the threshold, each under its field.
    draft.texts.insert("epoch".to_string(), "42".to_string());
    draft.texts.insert("finality.threshold".to_string(), "1".to_string());
    let built = build("switch_finality", &draft, Some(&stands), &env);
    assert!(built.errors["epoch"].starts_with("At least epoch 43"), "{:?}", built.errors);
    assert!(built.errors["finality.threshold"].contains("more than half"), "{:?}", built.errors);

    // Into FFG it names no validators, whatever stood.
    let stands = json!({ "epoch": null, "finality": change_json(&Change::Finality(authority()))["finality"] });
    let into = Env { finality: Some("authority"), ..env };
    let draft = Draft::of("switch_finality", Some(&stands), &into);
    assert_eq!(draft.text("finality.type"), "ffg");
    let built = build("switch_finality", &draft, Some(&stands), &into);
    assert!(matches!(&built.change, Some(Change::SwitchFinality { config: FinalityConfig::Ffg(config), .. }) if config.validators.is_empty()), "{:?}", built.errors);

    // A consensus that does not fit the new kind, and one on its way already.
    let staked = Env { consensus: Some("staked"), ..env };
    let stands = json!({ "epoch": null, "finality": change_json(&Change::Finality(ffg(None)))["finality"] });
    let mut draft = Draft::of("switch_finality", Some(&stands), &staked);
    draft.tables.get_mut("finality.signers").unwrap().push(vec![key(1).to_string(), address(1).to_string()]);
    assert!(build("switch_finality", &draft, Some(&stands), &staked).errors[GENERAL].contains("Validators by stake"));

    let switching = Env { switching: true, ..env };
    assert!(build("switch_finality", &draft, Some(&stands), &switching).errors[GENERAL].contains("on its way"));
    assert!(build("consensus", &Draft::of("consensus", None, &switching), None, &switching).errors[GENERAL].contains("on its way"));
}

#[test]
fn a_consensus_must_fit_the_finality_the_chain_runs() {
    let pow = json!({ "type": "pow", "hash": "blake3", "target_block_ms": 2000, "initial_difficulty": "100000", "min_difficulty": "1000" });
    let mut draft = Draft::of("consensus", Some(&pow), &env());
    draft.texts.insert("type".to_string(), "rotating".to_string());

    assert!(build("consensus", &draft, Some(&pow), &env()).errors["type"].contains("authority"));
    assert_eq!(build("consensus", &draft, Some(&pow), &Env { finality: Some("authority"), ..env() }).change, Some(Change::Consensus(ConsensusConfig::Rotating { slot_ms: 2_000 })));

    draft.texts.insert("type".to_string(), "staked".to_string());
    assert_eq!(build("consensus", &draft, Some(&pow), &env()).change, Some(Change::Consensus(ConsensusConfig::Staked { slot_ms: 2_000, schedule: Schedule::Draw })));
    assert!(build("consensus", &draft, Some(&pow), &Env { finality: Some("authority"), ..env() }).errors["type"].contains("FFG"));
}

#[test]
fn the_form_draws_at_a_phone_s_width_for_every_kind() {
    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(340.0, 4000.0));
    let env = env();
    let palette = theme::DARK;

    for change in every_shape() {
        let kind = change.name();
        let value = change_json(&change)[kind].clone();
        let mut draft = Draft::of(kind, Some(&value), &env);
        let built = build(kind, &draft, Some(&value), &env);
        let mut widest: f32 = 0.0;

        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| {
                widest = ui.scope(|ui| draw(ui, &palette, kind, &mut draft, Some(&value), &env, &built)).response.rect.right();
            });
            output.textures_delta.clear();
        }

        assert!(widest <= 340.5, "{kind} runs off a phone: {widest}");
    }
}

/// A network governance defines whole, Hoodi's under another name, as SSZ.
fn ethlight_network() -> Vec<u8> {
    let mut network = chainv4::ethlight::HOODI.clone();
    network.name = "private".into();

    network.encode()
}
