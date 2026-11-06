//! Genesis files: a chain's whole configuration as JSON, so every parameter can be chosen without
//! touching code. `chainv4 genesis` writes one with the development defaults; `chainv4 --genesis
//! FILE` runs a node on it. The JSON itself is crate::chain::json's, and a change's params::json's.

// A build without any kind of governance has an empty GovernanceConfig, matched on nothing.
#![cfg_attr(not(any(feature = "governance-electorate", feature = "governance-council")), allow(unreachable_code, unused_variables))]

#[cfg(feature = "finality-authority")]
use crate::chain::AuthorityConfig;
#[cfg(feature = "finality-depth")]
use crate::chain::DepthConfig;
#[cfg(feature = "finality-ffg")]
use crate::chain::FfgConfig;
use crate::governance::GovernanceConfig;
use {
    super::CliResult,
    crate::chain::fees::{BaseFee, Bid, ContractScorer, PricingConfig, ScheduleConfig, Weighted},
    crate::chain::VoteRewardSource,
    crate::chain::{FeeConfig, FinalityConfig, Genesis},
    crate::structs::address::Address,
    crate::registry::names::NamesConfig,
    serde_json::Value,
    std::path::{Path, PathBuf},
};

pub use crate::chain::json::*;

#[derive(clap::Args)]
pub struct GenesisArgs {
    /// Which development chain to start from.
    #[arg(long, value_enum, default_value_t = DevChain::Pow)]
    pub consensus: DevChain,
    /// Which finality to give it: ffg, authority, depth or none. Without one, the dev chains' own:
    /// FFG with the development key as its one validator.
    #[arg(long, value_parser = parse_finality)]
    pub finality: Option<FinalityKind>,
    /// What transactions pay: base-fee, the default, where a base price follows how full blocks
    /// are; bid, where senders name their price; or free.
    #[arg(long, value_enum, default_value_t = FeeKind::BaseFee)]
    pub fees: FeeKind,
    /// A contract whose score function says what each transaction is charged, on top of the
    /// weighted gas schedule. Deploy it once the chain runs; until then the weights alone count.
    #[arg(long)]
    pub scorer: Option<Address>,
    /// Which kinds of transaction the scorer is called for, such as call,deploy; the rest are
    /// charged as weighed, with no call. Without it, every kind.
    #[arg(long, requires = "scorer", value_delimiter = ',')]
    pub scorer_kinds: Vec<String>,
    /// Where vote rewards come from: mint, the default, or fees, where the burned share of every
    /// fee feeds a pool voters are paid from.
    #[arg(long)]
    pub vote_rewards: Option<VoteRewardSource>,
    /// Governance, so the chain can change its consensus by transaction: electorate, where
    /// finality's own validators or signers approve; council, where the development key alone
    /// does, to edit; or none, the default.
    #[arg(long, value_parser = parse_governance)]
    pub governance: Option<GovernanceChoice>,
    /// What the chain's names end in, such as dev for alice.dev, or none for a chain without
    /// names. Prices and the grace period are the dev chains'; edit the file for others.
    #[arg(long, default_value = "dev")]
    pub names: String,
    /// Where to write it. Without one, it goes to stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum DevChain {
    Pow,
    Authority,
    #[cfg(feature = "finality-authority")]
    Rotating,
    #[cfg(feature = "finality-ffg")]
    Staked,
}

/// A kind of governance to write a genesis with, with development settings.
#[derive(Clone, Copy)]
pub enum GovernanceChoice {
    #[cfg(feature = "governance-electorate")]
    Electorate,
    #[cfg(feature = "governance-council")]
    Council,
    None,
}

fn parse_governance(text: &str) -> Result<GovernanceChoice, String> {
    match text {
        #[cfg(feature = "governance-electorate")]
        "electorate" => Ok(GovernanceChoice::Electorate),
        #[cfg(feature = "governance-council")]
        "council" => Ok(GovernanceChoice::Council),
        "none" => Ok(GovernanceChoice::None),
        other => Err(format!("unknown governance {other}; use {}", GovernanceConfig::KINDS.iter().chain(["none"].iter()).copied().collect::<Vec<_>>().join(", "))),
    }
}

/// Governance with development settings: approvals count for 64 epochs, and a council is the
/// development key alone.
pub fn dev_governance(choice: GovernanceChoice) -> Option<GovernanceConfig> {
    match choice {
        #[cfg(feature = "governance-electorate")]
        GovernanceChoice::Electorate => Some(GovernanceConfig::Electorate { approval_epochs: 64, quorum_permille: 0 }),
        #[cfg(feature = "governance-council")]
        GovernanceChoice::Council => Some(GovernanceConfig::Council {
            members: vec![crate::consensus::DevAuthority::dev_keypair().public().address()],
            threshold: 1,
            approval_epochs: 64,
            quorum_permille: 0,
        }),
        GovernanceChoice::None => None,
    }
}

/// A kind of finality to write a genesis with, each with development settings.
#[derive(Clone, Copy)]
pub enum FinalityKind {
    #[cfg(feature = "finality-ffg")]
    Ffg,
    #[cfg(feature = "finality-authority")]
    Authority,
    #[cfg(feature = "finality-depth")]
    Depth,
    None,
}

fn parse_finality(text: &str) -> Result<FinalityKind, String> {
    match text {
        #[cfg(feature = "finality-ffg")]
        "ffg" => Ok(FinalityKind::Ffg),
        #[cfg(feature = "finality-authority")]
        "authority" => Ok(FinalityKind::Authority),
        #[cfg(feature = "finality-depth")]
        "depth" => Ok(FinalityKind::Depth),
        "none" => Ok(FinalityKind::None),
        other => Err(format!("unknown finality {other}; {}", finality_kinds())),
    }
}

/// A kind of finality with development settings, the development key voting.
pub fn dev_finality(kind: FinalityKind) -> Option<FinalityConfig> {
    #[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
    let dev = crate::consensus::DevAuthority::dev_keypair().public();

    match kind {
        #[cfg(feature = "finality-ffg")]
        FinalityKind::Ffg => Some(FinalityConfig::Ffg(FfgConfig::dev(dev))),
        #[cfg(feature = "finality-authority")]
        FinalityKind::Authority => Some(FinalityConfig::Authority(AuthorityConfig::dev(dev))),
        #[cfg(feature = "finality-depth")]
        FinalityKind::Depth => Some(FinalityConfig::Depth(DepthConfig::dev())),
        FinalityKind::None => None,
    }
}

/// How the written genesis charges for transactions.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum FeeKind {
    BaseFee,
    Bid,
    Free,
}

/// Development fees of a kind, with a scorer contract if one is named, scoring these kinds of
/// transaction, or every kind when none are named.
pub fn dev_fees(kind: FeeKind, scorer: Option<Address>, kinds: Vec<String>) -> Option<FeeConfig> {
    let pricing = match kind {
        FeeKind::BaseFee => PricingConfig::BaseFee(BaseFee::dev()),
        FeeKind::Bid => PricingConfig::Bid(Bid::dev()),
        FeeKind::Free => return None,
    };
    let schedule = match scorer {
        Some(contract) => ScheduleConfig::Contract(ContractScorer {
            weights: Weighted::dev(),
            contract,
            fuel: 100_000,
            kinds,
        }),
        None => ScheduleConfig::Weighted(Weighted::dev()),
    };

    Some(FeeConfig {
        schedule,
        pricing,
        ..FeeConfig::dev()
    })
}

/// The development genesis of a kind, as the node runs it without --genesis.
pub fn dev(chain: DevChain) -> Genesis {
    let dev = crate::consensus::DevAuthority::dev_keypair().public();

    match chain {
        DevChain::Pow => Genesis::dev_pow(dev),
        DevChain::Authority => Genesis::dev_authority(dev),
        #[cfg(feature = "finality-authority")]
        DevChain::Rotating => Genesis::dev_rotating(dev),
        #[cfg(feature = "finality-ffg")]
        DevChain::Staked => Genesis::dev_staked(dev),
    }
}

/// Writes a development genesis to edit.
pub fn write(args: GenesisArgs) -> CliResult<()> {
    let mut genesis = dev(args.consensus);

    if let Some(kind) = args.finality {
        genesis.finality = dev_finality(kind);
    }

    if args.scorer.is_some() && matches!(args.fees, FeeKind::Free) {
        return Err("--scorer needs fees; leave out --fees free".into());
    }

    genesis.fees = dev_fees(args.fees, args.scorer, args.scorer_kinds);
    genesis.vote_reward_source = args.vote_rewards;

    if let Some(choice) = args.governance {
        genesis.governance = dev_governance(choice);
    }

    genesis.names = match args.names.as_str() {
        "none" => None,
        suffix => Some(NamesConfig { suffix: suffix.to_string(), ..NamesConfig::dev() }),
    };

    if let Some(problem) = genesis.names.as_ref().and_then(NamesConfig::problem) {
        return Err(format!("--names: {problem}").into());
    }

    let text = serde_json::to_string_pretty(&to_json(&genesis))? + "\n";

    match args.out {
        Some(path) => {
            std::fs::write(&path, text).map_err(|err| format!("cannot write {}: {err}", path.display()))?;
            println!("wrote {}", path.display());
            println!("genesis {}", genesis.block().id());
        }
        None => print!("{text}"),
    }

    Ok(())
}

/// Reads a genesis file.
pub fn read(path: &Path) -> CliResult<Genesis> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    let value: Value = serde_json::from_str(&text).map_err(|err| format!("{} is not JSON: {err}", path.display()))?;

    from_json(&value).map_err(|err| format!("{}: {err}", path.display()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "secp256k1")]
    use crate::registry::federation::Federation;
    use {
        crate::chain::{Allocation, Coin, Reward},
        crate::consensus::ConsensusConfig,
        crate::registry::collections::CollectionFunctions,
        crate::registry::directory::{Directory, EvmGateway},
        crate::structs::code::Engine,
        crate::structs::hash::HashType,
        crate::structs::u256::u256,
        crate::vm::Limits,
        serde_json::json,
    };

    /// Every finality a genesis can have in this build, none included.
    const ALL_FINALITY: &[FinalityKind] = &[
        #[cfg(feature = "finality-ffg")]
        FinalityKind::Ffg,
        #[cfg(feature = "finality-authority")]
        FinalityKind::Authority,
        #[cfg(feature = "finality-depth")]
        FinalityKind::Depth,
        FinalityKind::None,
    ];

    #[test]
    fn bootnodes_round_trip_and_leave_a_chain_without_them_its_id() {
        let plain = dev(DevChain::Pow);
        let mut booted = plain.clone();
        booted.bootnodes = vec!["/ip4/203.0.113.7/udp/4001/quic-v1".to_string()];
        let value = to_json(&booted);

        assert_eq!(from_json(&value).unwrap(), booted);
        assert_ne!(booted.block().id(), plain.block().id(), "a chain's bootnodes are part of it");

        // A file from before the field reads as none, and keeps its id.
        let mut without = to_json(&plain);
        without.as_object_mut().unwrap().remove("bootnodes");

        assert_eq!(from_json(&without).unwrap().block().id(), plain.block().id());

        booted.bootnodes.push("203.0.113.8:4001".to_string());

        assert!(crate::chain::Chain::open(std::sync::Arc::new(crate::db::MemoryDb::default()), &booted).is_err());
    }

    fn with(kind: FinalityKind) -> Value {
        let mut genesis = dev(DevChain::Authority);
        genesis.finality = dev_finality(kind);

        to_json(&genesis)
    }

    #[cfg(all(feature = "finality-ffg", feature = "governance-electorate", feature = "governance-council"))]
    #[test]
    fn governance_round_trips_and_is_checked() {
        use crate::db::MemoryDb;
        use std::sync::Arc;

        for choice in [GovernanceChoice::Electorate, GovernanceChoice::Council, GovernanceChoice::None] {
            let mut genesis = dev(DevChain::Authority);
            genesis.governance = dev_governance(choice);
            let value = to_json(&genesis);
            let read = from_json(&value).unwrap();

            assert_eq!(read, genesis);
            assert_eq!(read.block().id(), genesis.block().id());
            assert_eq!(value["governance"].is_null(), matches!(choice, GovernanceChoice::None));
        }

        // A file from before the field reads as none, and keeps its id.
        let mut without = to_json(&dev(DevChain::Authority));
        without.as_object_mut().unwrap().remove("governance");

        assert_eq!(from_json(&without).unwrap(), dev(DevChain::Authority));

        // Governance needs finality that votes; a council needs members and a threshold they can
        // reach; unknown kinds are refused by name.
        let mut depthless = dev(DevChain::Authority);
        depthless.finality = None;
        depthless.governance = dev_governance(GovernanceChoice::Electorate);

        let refused = |genesis: &Genesis| match crate::chain::Chain::open(Arc::new(MemoryDb::default()), genesis) {
            Ok(_) => panic!("opened a chain genesis should refuse"),
            Err(err) => err.to_string(),
        };

        assert!(refused(&depthless).contains("finality that votes"));

        let mut empty = dev(DevChain::Authority);
        empty.governance = Some(GovernanceConfig::Council { members: Vec::new(), threshold: 1, approval_epochs: 64, quorum_permille: 0 });

        assert!(refused(&empty).contains("at least one account"));

        let mut unknown = to_json(&dev(DevChain::Authority));
        unknown["governance"] = json!({ "type": "plutocracy" });

        assert!(from_json(&unknown).unwrap_err().contains("unknown governance plutocracy"));
        assert_eq!(parse_governance("council").map(|choice| dev_governance(choice).unwrap().name()), Ok("council"));
        assert!(parse_governance("plutocracy").is_err());
    }

    #[test]
    fn genesis_files_round_trip_exactly() {
        let scorer = Address::Contract([5; 32]);
        let fee_kinds = [
            (FeeKind::BaseFee, None),
            (FeeKind::Bid, None),
            (FeeKind::BaseFee, Some(scorer)),
            (FeeKind::Free, None),
        ];

        let chains = [
            DevChain::Pow,
            DevChain::Authority,
            #[cfg(feature = "finality-authority")]
            DevChain::Rotating,
            #[cfg(feature = "finality-ffg")]
            DevChain::Staked,
        ];

        for chain in chains {
            for &kind in ALL_FINALITY {
                for (fees, scorer) in fee_kinds {
                    let mut genesis = dev(chain);
                    genesis.finality = dev_finality(kind);
                    genesis.fees = dev_fees(fees, scorer, Vec::new());
                    let read = from_json(&to_json(&genesis)).unwrap();

                    assert_eq!(read, genesis);
                    assert_eq!(read.block().id(), genesis.block().id());
                }
            }
        }
    }

    #[test]
    fn a_genesis_from_before_max_block_bytes_keeps_its_id() {
        let mut genesis = dev(DevChain::Authority);
        genesis.max_block_bytes = None;
        // The EVM, names and collections came later, and joined the dev chains then.
        genesis.engines.retain(|engine| *engine != Engine::Evm);
        genesis.names = None;
        genesis.collections = None;
        genesis.federation = None;
        genesis.directory = None;
        let mut value = to_json(&genesis);
        value.as_object_mut().unwrap().remove("max_block_bytes");
        let read = from_json(&value).unwrap();

        /// Genesis as it was before the field, field for field.
        #[derive(wincode::SchemaWrite)]
        struct Before {
            timestamp: u64,
            hash: HashType,
            coin: Coin,
            consensus: ConsensusConfig,
            reward: Reward,
            minter: Option<Address>,
            allocations: Vec<Allocation>,
            engines: Vec<Engine>,
            tx_limits: Limits,
            token_functions: Vec<String>,
            max_reorg_depth: u64,
            finality: Option<FinalityConfig>,
            fees: Option<FeeConfig>,
        }

        let before = Before {
            timestamp: read.timestamp,
            hash: read.hash,
            coin: read.coin.clone(),
            consensus: read.consensus.clone(),
            reward: read.reward,
            minter: read.minter,
            allocations: read.allocations.clone(),
            engines: read.engines.clone(),
            tx_limits: read.tx_limits,
            token_functions: read.token_functions.clone(),
            max_reorg_depth: read.max_reorg_depth,
            finality: read.finality.clone(),
            fees: read.fees.clone(),
        };

        // Unset, it adds nothing to what the id is hashed from, in any build.
        assert_eq!(read.max_block_bytes, None, "a file without it reads as unbounded");
        assert_eq!(read.pow_window, None, "nor does a window, which the authority chain has none of");
        assert_eq!(read.vote_reward_source, None, "nor minted vote rewards");
        assert_eq!(wincode::serialize(&read).unwrap(), wincode::serialize(&before).unwrap());
        assert_eq!(from_json(&to_json(&read)).unwrap(), read, "null reads back as unset too");

        // The development authority chain's id from before the field, in the build it was taken
        // from: its hash, key, engines and finality all go into it. Taken again for format 9,
        // whose headers commit to their payouts too, which moved every block's id; and again for
        // the coin's supply, kept in state from genesis on, which moved every state root; and again
        // for FFG's slash_permille, which every FFG config encodes, set or not; and again for fees'
        // lua_costs, which every fee config encodes, set or not.
        if cfg!(all(
            feature = "blake3",
            feature = "ed25519",
            feature = "lvm",
            feature = "wasm",
            feature = "finality-ffg",
            feature = "finality-authority",
            feature = "finality-depth"
        )) {
            assert_eq!(read.block().id().to_string(), "0xf707153c93394fffc2619bbfee37f8945aae0585b7b646b3830a713910178fa8");
        }

        // Set, it is part of the chain: the dev chains now have their own ids.
        let bounded = dev(DevChain::Authority);

        assert_eq!(bounded.max_block_bytes, Some(crate::chain::DEV_MAX_BLOCK_BYTES));
        assert_ne!(bounded.block().id(), read.block().id());
        assert_eq!(from_json(&to_json(&bounded)).unwrap(), bounded);

        let mut other = bounded.clone();
        other.max_block_bytes = Some(8 * 1024 * 1024);

        assert_ne!(other.block().id(), bounded.block().id());
    }

    #[test]
    fn every_parameter_can_change_and_changes_the_chain() {
        let genesis = dev(DevChain::Pow);
        let mut value = to_json(&genesis);
        value["tx_limits"]["fuel"] = json!(1_000);
        value["max_reorg_depth"] = json!(500);
        let changed = from_json(&value).unwrap();

        assert_eq!((changed.tx_limits.fuel, changed.max_reorg_depth), (1_000, 500));
        assert_ne!(changed.block().id(), genesis.block().id());

        // The chain hash and the work hash can each be any secure one this build has.
        for &algorithm in HashType::ALL.iter().filter(|algorithm| algorithm.secure()) {
            let mut value = to_json(&genesis);
            value["hash"] = json!(algorithm.name());
            value["consensus"]["hash"] = json!(algorithm.name());
            let read = from_json(&value).unwrap();

            assert_eq!(read.hash, algorithm);
            assert_eq!(read.block().id().algorithm(), algorithm);
        }
    }

    #[cfg(feature = "secp256k1")]
    #[test]
    fn a_federation_round_trips_and_a_file_without_one_keeps_its_id() {
        let federation = Federation {
            signers: (1..=3u8).map(|byte| crate::crypto::secp256k1::public_key(&[byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
            btc: None,
        };
        let genesis = Genesis { federation: Some(federation.clone()), ..dev(DevChain::Pow) };
        let value = to_json(&genesis);

        assert_eq!(value["federation"]["threshold"], 2);
        assert_eq!(from_json(&value).unwrap(), genesis);
        assert_ne!(genesis.block().id(), Genesis { federation: None, ..genesis.clone() }.block().id());

        let mut without = value.clone();
        without.as_object_mut().unwrap().remove("federation");

        assert_eq!(from_json(&without).unwrap().block().id(), Genesis { federation: None, ..genesis.clone() }.block().id());

        // vote-change's flag reads the same JSON, and checks it as genesis would.
        assert_eq!(federation_from_text(&value["federation"].to_string()).unwrap(), federation);

        let mut short = value["federation"].clone();
        short["threshold"] = json!(4);

        assert!(federation_from_text(&short.to_string()).unwrap_err().contains("threshold"));
        short["signers"] = json!(["0x1234"]);

        assert!(federation_from_text(&short.to_string()).unwrap_err().contains("compressed key"));
    }

    #[test]
    fn approve_change_reads_fees_a_reward_limits_and_finality_as_genesis_writes_them() {
        let genesis = dev(DevChain::Authority);
        let value = to_json(&genesis);

        // Fees and finality as a genesis file has them, a reward and limits in genesis's shape.
        assert_eq!(fees_from_text(&value["fees"].to_string()).unwrap(), genesis.fees.clone().unwrap());
        assert_eq!(reward_from_text(&value["reward"].to_string()).unwrap(), genesis.reward);
        assert_eq!(reward_from_text(&reward_json(&genesis.reward).to_string()).unwrap(), genesis.reward);
        let limits = crate::chain::BlockLimits { tx_limits: genesis.tx_limits, max_block_bytes: Some(2 << 20), contract: None, max_tx_bytes: None, multisig_members: None };
        assert_eq!(limits_from_text(&limits_json(&limits).to_string()).unwrap(), limits);

        if let Some(finality) = &genesis.finality {
            assert_eq!(&finality_from_text(&value["finality"].to_string()).unwrap(), finality);
        }

        // And each checked as genesis is.
        assert!(reward_from_text(r#"{"initial":"1","halving_interval":0}"#).unwrap_err().contains("halves"));
        assert!(limits_from_text(r#"{"tx_limits":{"fuel":1,"memory":1},"max_block_bytes":null}"#).unwrap_err().contains("fuel"));
        let mut broke = value["fees"].clone();
        broke["block_gas_limit"] = json!(0);
        assert!(fees_from_text(&broke.to_string()).unwrap_err().contains("block_gas_limit"));
    }

    #[test]
    fn a_directory_admin_round_trips_and_a_file_without_one_keeps_its_id() {
        let admin = crate::consensus::DevAuthority::dev_keypair().public().address();
        let genesis = Genesis { directory_admin: Some(admin), ..dev(DevChain::Pow) };
        let value = to_json(&genesis);

        assert_eq!(value["directory_admin"], admin.to_string());
        assert_eq!(from_json(&value).unwrap(), genesis);
        assert_ne!(genesis.block().id(), Genesis { directory_admin: None, ..genesis.clone() }.block().id(), "the admin is part of the chain's id");

        let mut without = value.clone();
        without.as_object_mut().unwrap().remove("directory_admin");

        assert_eq!(from_json(&without).unwrap().block().id(), Genesis { directory_admin: None, ..genesis.clone() }.block().id(), "a file from before names none, and keeps its id");

        let mut bad = value.clone();
        bad["directory_admin"] = json!("not an address");

        assert!(from_json(&bad).is_err());
    }

    #[test]
    fn a_name_round_trips_changes_the_chain_lands_in_state_and_a_file_without_one_has_none() {
        let genesis = Genesis { name: Some("chainv4 testnet".to_string()), ..dev(DevChain::Pow) };
        let value = to_json(&genesis);

        assert_eq!(value["name"], "chainv4 testnet");
        assert_eq!(from_json(&value).unwrap(), genesis);
        assert_ne!(genesis.block().id(), Genesis { name: None, ..genesis.clone() }.block().id());

        let mut without = value.clone();
        without.as_object_mut().unwrap().remove("name");

        assert_eq!(from_json(&without).unwrap().block().id(), Genesis { name: None, ..genesis.clone() }.block().id(), "a file from before names none, and keeps its id");

        let chain = crate::chain::Chain::open(std::sync::Arc::new(crate::db::MemoryDb::default()), &genesis).unwrap();

        assert_eq!(crate::registry::changes::Changes::new(&chain.state()).name().unwrap().as_deref(), Some("chainv4 testnet"));

        for bad in ["", " padded", "line\nbreak", &"x".repeat(41)] {
            let bad = Genesis { name: Some(bad.to_string()), ..dev(DevChain::Pow) };

            assert!(crate::chain::Chain::open(std::sync::Arc::new(crate::db::MemoryDb::default()), &bad).is_err(), "{:?}", bad.name);
        }
    }

    #[test]
    fn a_directory_round_trips_changes_the_chain_and_a_file_without_one_announces_nothing() {
        let directory = Directory {
            gateway: Some(Address::Contract([1; 32])),
            evm_gateways: vec![EvmGateway { chain_id: 1, gateway: [3; 20] }, EvmGateway { chain_id: 8453, gateway: [4; 20] }],
            swap: Some(Address::Contract([5; 32])),
            dex_router: Some([7; 20]),
            ..Directory::default()
        };
        let genesis = Genesis { directory: Some(directory.clone()), ..dev(DevChain::Pow) };
        let value = to_json(&genesis);

        assert_eq!(value["directory"]["evm_gateways"][1]["chain_id"], 8453);
        assert_eq!((&value["directory"]["btc_gateway"], &value["directory"]["dex_router"]), (&Value::Null, &json!(format!("0x{}", "07".repeat(20)))));
        assert_eq!(from_json(&value).unwrap(), genesis);
        assert_ne!(genesis.block().id(), Genesis { directory: None, ..genesis.clone() }.block().id());

        let mut without = value.clone();
        without.as_object_mut().unwrap().remove("directory");

        assert_eq!(from_json(&without).unwrap().block().id(), Genesis { directory: None, ..genesis.clone() }.block().id());

        // vote-change's flag reads the same JSON, and checks it as genesis would.
        assert_eq!(directory_from_text(&value["directory"].to_string()).unwrap(), directory);

        let mut wrong = value["directory"].clone();
        wrong["evm_gateways"] = json!([{ "chain_id": 5, "gateway": "0x1234" }]);

        assert!(directory_from_text(&wrong.to_string()).unwrap_err().contains("EVM address"));
        wrong["evm_gateways"] = json!([{ "chain_id": 5, "gateway": format!("0x{}", "11".repeat(20)) }, { "chain_id": 1, "gateway": format!("0x{}", "22".repeat(20)) }]);

        assert!(directory_from_text(&wrong.to_string()).unwrap_err().contains("order"));
        wrong["evm_gateways"] = json!([]);
        wrong["swap"] = json!(Address::Ed25519([1; 32]).to_string());

        assert!(directory_from_text(&wrong.to_string()).unwrap_err().contains("contracts"));
    }

    #[test]
    fn collections_round_trip_change_the_chain_and_a_file_without_them_has_none() {
        let genesis = dev(DevChain::Pow);
        let value = to_json(&genesis);

        assert_eq!(value["collections"]["nft"][4], "owner_of");
        assert_eq!(from_json(&value).unwrap(), genesis);

        // Without the field, as files from before it: no registry, and the id of a genesis without.
        let mut without = value.clone();
        without.as_object_mut().unwrap().remove("collections");
        let read = from_json(&without).unwrap();

        assert_eq!(read.collections, None);
        assert_eq!(read.block().id(), Genesis { collections: None, ..genesis.clone() }.block().id());
        assert_ne!(read.block().id(), genesis.block().id());

        // Its shape is read here; a function named twice is the chain's to refuse, as for tokens.
        for bad in [json!({ "nft": [] }), json!({ "nft": [], "multi": [], "more": [] }), json!({ "nft": "owner_of", "multi": [] }), json!(["owner_of"])] {
            let mut broken = value.clone();
            broken["collections"] = bad.clone();

            assert!(from_json(&broken).is_err(), "{bad}");
        }

        // vote-change's flag: standard, or the same JSON.
        assert_eq!(collections_from_text("standard").unwrap(), CollectionFunctions::standard());
        assert_eq!(collections_from_text(r#"{"nft":["owner_of"],"multi":[]}"#).unwrap().nft, ["owner_of"]);
        assert!(collections_from_text("nonsense").is_err());
        assert!(collections_from_text(r#"{"nft":["",""],"multi":[]}"#).is_err());
    }

    #[cfg(feature = "names")]
    #[test]
    fn names_round_trip_change_the_chain_and_a_file_without_them_has_none() {
        let genesis = dev(DevChain::Pow);
        let value = to_json(&genesis);

        assert_eq!(value["names"]["suffix"], "dev");
        assert_eq!(from_json(&value).unwrap(), genesis);

        let mut other = value.clone();
        other["names"]["suffix"] = json!("kyn");
        other["names"]["prices"] = json!(["7"]);
        let read = from_json(&other).unwrap();

        assert_eq!(read.names.as_ref().map(|names| (names.suffix.as_str(), names.prices.clone())), Some(("kyn", vec![u256::from(7u64)])));
        assert_ne!(read.block().id(), genesis.block().id());

        // Without the field, as files from before it: no names, and the id of a genesis without.
        let mut without = value.clone();
        without.as_object_mut().unwrap().remove("names");
        let read = from_json(&without).unwrap();

        assert_eq!(read.names, None);
        assert_eq!(read.block().id(), Genesis { names: None, ..genesis.clone() }.block().id());

        for (field, bad) in [("prices", json!([5])), ("prices", json!("5")), ("grace_ms", json!("90 days")), ("max_years", json!(5_000_000_000u64))] {
            let mut broken = value.clone();
            broken["names"][field] = bad;

            assert!(from_json(&broken).is_err(), "{field}");
        }

        let mut extra = value.clone();
        extra["names"]["owner"] = json!("me");

        assert_eq!(from_json(&extra).unwrap_err(), "names.owner is not a genesis field");
    }

    #[test]
    fn each_finality_kind_reads_its_own_parameters() {
        #[cfg(feature = "finality-ffg")]
        {
            let mut value = with(FinalityKind::Ffg);
            value["finality"]["checkpoint_interval"] = json!(32);
            value["finality"]["activation_delay"] = json!(4);
            let Some(FinalityConfig::Ffg(ffg)) = from_json(&value).unwrap().finality else {
                panic!("not ffg")
            };

            assert_eq!((ffg.checkpoint_interval, ffg.activation_delay), (32, 4));
        }

        #[cfg(feature = "finality-authority")]
        {
            let scheme = crate::consensus::DevAuthority::dev_keypair().public().key_type();
            let signers: Vec<crate::crypto::keypair::PublicKey> = (0..3)
                .map(|_| crate::crypto::keypair::Keypair::generate(scheme).unwrap().public())
                .collect();
            let mut value = with(FinalityKind::Authority);
            value["finality"]["signers"] = json!(signers.iter().map(ToString::to_string).collect::<Vec<_>>());
            value["finality"]["threshold"] = json!(2);
            let Some(FinalityConfig::Authority(authority)) = from_json(&value).unwrap().finality else {
                panic!("not authority")
            };

            assert_eq!((authority.signers.iter().map(|signer| signer.key).collect::<Vec<_>>(), authority.threshold), (signers.clone(), 2));
            assert!(authority.signers.iter().all(|signer| signer.account == signer.key.address()), "a bare key is its own account");

            // An object names the account that speaks for the key.
            let owner = crate::crypto::keypair::Keypair::generate(scheme).unwrap().public().address();
            value["finality"]["signers"][0] = json!({ "key": signers[0].to_string(), "account": owner.to_string() });
            let Some(FinalityConfig::Authority(authority)) = from_json(&value).unwrap().finality else {
                panic!("not authority")
            };

            assert_eq!(authority.signers[0].account, owner);
            assert_eq!(from_json(&to_json(&from_json(&value).unwrap())).unwrap(), from_json(&value).unwrap());
        }

        #[cfg(feature = "finality-depth")]
        {
            let mut value = with(FinalityKind::Depth);
            value["finality"]["depth"] = json!(12);

            assert_eq!(
                from_json(&value).unwrap().finality,
                Some(FinalityConfig::Depth(DepthConfig { depth: 12 }))
            );
        }

        // The finality's type is required, and must be a kind this build has.
        for &kind in ALL_FINALITY {
            let mut value = with(kind);

            if value["finality"].is_null() {
                continue;
            }

            value["finality"]["type"] = json!("grandpa");

            assert!(from_json(&value).unwrap_err().contains("unknown finality grandpa"));

            value["finality"].as_object_mut().unwrap().remove("type");

            assert_eq!(from_json(&value).unwrap_err(), "finality.type is missing");
        }
    }

    #[test]
    fn mistakes_are_named() {
        let cases = [
            #[cfg(feature = "finality-ffg")]
            ("/finality/checkpont_interval", json!(8), "finality.checkpont_interval is not a genesis field"),
            ("/max_reorg_depth", json!("100"), "max_reorg_depth must be a whole number"),
            ("/max_block_bytes", json!("4 MiB"), "max_block_bytes must be a whole number"),
            ("/engines", json!(["lua", "solidity"]), "unknown engine solidity"),
            ("/token_functions", json!("transfer"), "token_functions must be a list"),
            ("/consensus/hash", json!("sha1"), "consensus.hash: unknown hash sha1"),
            ("/allocations/0/amount", json!("-5"), "allocations[0].amount: -5 is not a whole amount"),
            ("/fees/pricing/type", json!("auction"), "unknown pricing auction"),
            ("/fees/schedule/per_byte", json!(-1), "fees.schedule.per_byte must be a whole number"),
            ("/fees/burn_permille", json!(70_000), "fees.burn_permille is too large"),
            ("/coin/decimals", json!(300), "coin.decimals is too large"),
            ("/coin/name", json!("Dev"), "coin.name is not a genesis field"),
        ];

        for (pointer, replacement, expected) in cases {
            let mut value = to_json(&dev(DevChain::Pow));
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            let target = if parent.is_empty() { &mut value } else { value.pointer_mut(parent).unwrap() };

            match target {
                Value::Object(map) => {
                    map.insert(key.to_string(), replacement);
                }
                Value::Array(items) => items[key.parse::<usize>().unwrap()] = replacement,
                _ => unreachable!(),
            }

            let err = from_json(&value).unwrap_err();

            assert!(err.contains(expected), "{pointer}: {err}");
        }

        #[cfg(feature = "finality-ffg")]
        {
            let mut missing = to_json(&dev(DevChain::Pow));
            missing["finality"].as_object_mut().unwrap().remove("vote_depth");

            assert_eq!(from_json(&missing).unwrap_err(), "finality.vote_depth is missing");
        }
    }

    #[test]
    fn part_of_a_setting_is_laid_over_what_stands() {
        let standing = fees_json(&FeeConfig::dev());
        let merged = over(&standing, &json!({ "burn_permille": 600 }));

        assert_eq!(merged["burn_permille"], 600);
        assert_eq!(merged["block_gas_limit"], standing["block_gas_limit"], "what is not named stands");
        assert_eq!(fees_from_text(&merged.to_string()).unwrap().burn_permille, 600, "and the whole is checked as a change is");

        // An object's fields one by one; anything else whole.
        assert_eq!(over(&json!({ "a": { "b": 1, "c": 2 }, "d": [1, 2] }), &json!({ "a": { "c": 3 }, "d": [9] })), json!({ "a": { "b": 1, "c": 3 }, "d": [9] }));
        assert_eq!(over(&Value::Null, &json!({ "x": 1 })), json!({ "x": 1 }), "nothing standing: what is given");
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

    #[cfg(feature = "governance-council")]
    #[test]
    fn locked_round_trips_and_a_file_without_it_keeps_its_id() {
        let mut genesis = dev(DevChain::Authority);
        genesis.governance = Some(GovernanceConfig::Council { members: vec![crate::consensus::DevAuthority::dev_keypair().public().address()], threshold: 1, approval_epochs: 64, quorum_permille: 0 });
        let plain = genesis.clone();
        genesis.locked = vec!["reward".to_string(), "anchoring.enforce".to_string()];
        let value = to_json(&genesis);

        assert_eq!(value["locked"], json!(["reward", "anchoring.enforce"]));
        assert_eq!(from_json(&value).unwrap(), genesis);
        assert_ne!(genesis.block().id(), plain.block().id(), "a chain's locks are part of it");

        let mut without = to_json(&plain);
        without.as_object_mut().unwrap().remove("locked");
        assert_eq!(from_json(&without).unwrap().block().id(), plain.block().id());
    }

    #[test]
    fn every_change_round_trips_through_its_json_and_bad_ones_are_refused() {
        use crate::governance::Change;

        let genesis = dev(DevChain::Pow);
        let mut changes = vec![
            Change::Consensus(genesis.consensus.clone()),
            Change::Bootnodes(vec!["/dns4/boot.example/udp/4001/quic-v1".to_string()]),
            Change::Names(crate::registry::names::NameSettings { prices: vec![u256::new(5), u256::new(1)], grace_ms: 1_000, max_years: 3 }),
            Change::TokenFunctions(vec!["balance_of".to_string(), "transfer".to_string()]),
            Change::EvmSpec { spec: crate::params::vm::EvmSpec::Osaka, from: 1_000 },
            Change::Dex(crate::params::vm::DexSettings { fee_bps: 25 }),
            Change::Collections(CollectionFunctions::standard()),
            Change::Directory(Directory { swap: Some(Address::Contract([7; 32])), ..Directory::default() }),
            Change::Fees(FeeConfig::dev()),
            Change::Reward(Reward { initial: u256::new(50), halving_interval: Some(1_000) }),
            Change::Limits(crate::chain::BlockLimits { tx_limits: genesis.tx_limits, max_block_bytes: Some(2 * 1024 * 1024), contract: None, max_tx_bytes: None, multisig_members: None }),
            Change::Upgrade(crate::structs::version::Upgrade { least: "0.2.0".parse().unwrap(), from: 1_000 }),
            Change::Name("chainv4 testnet".to_string()),
            Change::Locks(vec!["anchoring.enforce".to_string(), "reward".to_string()]),
        ];

        if let Some(finality) = FinalityConfig::dev(crate::consensus::DevAuthority::dev_keypair().public()) {
            changes.push(Change::Finality(finality.clone()));
            changes.push(Change::SwitchFinality { epoch: 12, config: finality });
        }

        #[cfg(feature = "secp256k1")]
        changes.push(Change::Federation(Federation {
            signers: (1..=3u8).map(|byte| crate::crypto::secp256k1::public_key(&[byte; 32]).unwrap()).collect(),
            threshold: 2,
            chain_depth: 20,
            evm_depth: 12,
            bitcoin_depth: 6,
            btc: None,
        }));

        for change in changes {
            let value = change_json(&change);

            assert_eq!(change_from_json(&value).as_ref(), Ok(&change), "{value}");

            // Every field a change writes is one a lock may name.
            let mut found = Vec::new();
            leaves(&value, String::new(), &mut found);

            for leaf in found {
                // A switch is locked whole, and its finality's fields are finality's.
                let leaf = match (leaf.strip_prefix("switch_finality.finality."), leaf.starts_with("switch_finality.")) {
                    (Some(rest), _) => format!("finality.{rest}"),
                    (None, true) => continue,
                    (None, false) => leaf,
                };

                assert_eq!(crate::governance::locks::path_problem(&leaf), None, "{leaf}");
            }
        }

        // A change of locks is put in order; one naming a path twice, none, or nothing, is not one.
        assert_eq!(change_from_json(&json!({ "locks": ["reward", "dex"] })), Ok(Change::Locks(vec!["dex".to_string(), "reward".to_string()])));

        for (bad, why) in [
            (json!({ "name": " spaced " }), "name"),
            (json!({ "bootnodes": ["not an address"] }), "bootnodes"),
            (json!({ "name_prices": [] }), "name_prices"),
            (json!({ "name": "a", "bootnodes": [] }), "one field"),
            (json!({ "colour": "blue" }), "not something governance changes"),
            (json!({ "upgrade": { "least": "0.2.0" } }), "upgrade.from"),
            (json!({ "locks": ["reward.nope"] }), "names nothing in reward"),
            (json!({ "locks": ["dex", "dex"] }), "each path once"),
            (json!({ "locks": [] }), "at least one"),
            (json!({ "locks": "dex" }), "a list of paths"),
        ] {
            let refused = change_from_json(&bad).unwrap_err();

            assert!(refused.contains(why), "{bad}: {refused}");
        }
    }
}
