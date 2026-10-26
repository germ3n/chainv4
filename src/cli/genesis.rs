//! Genesis files: a chain's whole configuration as JSON, so every parameter can be chosen without
//! touching code. `chainv4 genesis` writes one with the development defaults; `chainv4 --genesis
//! FILE` runs a node on it. Every field must be present, and unknown fields are refused, so a
//! misspelled parameter is an error rather than a silent default.

// A build without any kind of governance has an empty GovernanceConfig, matched on nothing.
#![cfg_attr(not(any(feature = "governance-electorate", feature = "governance-council")), allow(unreachable_code, unused_variables))]

#[cfg(feature = "finality-authority")]
use crate::chain::AuthorityConfig;
#[cfg(feature = "finality-depth")]
use crate::chain::DepthConfig;
#[cfg(feature = "finality-ffg")]
use crate::chain::{FfgConfig, GenesisValidator};
#[cfg(feature = "finality-ffg")]
use crate::crypto::keypair::PublicKey;
use crate::governance::GovernanceConfig;
use {
    super::CliResult,
    crate::chain::fees::{BaseFee, Bid, ContractScorer, PricingConfig, ScheduleConfig, Weighted},
    crate::chain::VoteRewardSource,
    crate::chain::{Allocation, Coin, FeeConfig, FinalityConfig, Genesis, Reward},
    crate::structs::address::Address,
    crate::consensus::{ConsensusConfig, PowConfig},
    crate::structs::hash::HashType,
    crate::structs::code::Engine,
    crate::structs::u256::u256,
    crate::vm::Limits,
    serde_json::{Map, Value, json},
    std::collections::HashSet,
    std::path::{Path, PathBuf},
};

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
        GovernanceChoice::Electorate => Some(GovernanceConfig::Electorate { approval_epochs: 64 }),
        #[cfg(feature = "governance-council")]
        GovernanceChoice::Council => Some(GovernanceConfig::Council {
            members: vec![crate::consensus::DevAuthority::dev_keypair().public().address()],
            threshold: 1,
            approval_epochs: 64,
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
        other => Err(format!("unknown finality {other}; {}", kinds())),
    }
}

/// The finality kinds this build has, for error messages.
fn kinds() -> String {
    match FinalityConfig::KINDS {
        [] => "this build has none".to_string(),
        kinds => format!("this build has {}", kinds.join(", ")),
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

pub fn to_json(genesis: &Genesis) -> Value {
    json!({
        "timestamp": genesis.timestamp,
        "hash": genesis.hash.name(),
        "coin": coin_json(&genesis.coin),
        "consensus": consensus_json(&genesis.consensus, genesis.pow_window),
        "reward": {
            "initial": genesis.reward.initial.to_string(),
            "halving_interval": genesis.reward.halving_interval,
        },
        "minter": genesis.minter.map(|minter| minter.to_string()),
        "allocations": genesis.allocations.iter().map(|allocation| json!({
            "address": allocation.address.to_string(),
            "amount": allocation.amount.to_string(),
        })).collect::<Vec<_>>(),
        "engines": genesis.engines.iter().map(|engine| engine.name()).collect::<Vec<_>>(),
        "tx_limits": {
            "fuel": genesis.tx_limits.fuel,
            "memory": genesis.tx_limits.memory,
        },
        "token_functions": genesis.token_functions,
        "max_reorg_depth": genesis.max_reorg_depth,
        "finality": genesis.finality.as_ref().map(finality_json),
        "fees": genesis.fees.as_ref().map(fees_json),
        "max_block_bytes": genesis.max_block_bytes,
        "vote_reward_source": genesis.vote_reward_source.map(|source| source.name()),
        "governance": genesis.governance.as_ref().map(governance_json),
    })
}

/// A consensus as JSON: the shape genesis files, GET /governance and approve-change use.
pub fn consensus_json(consensus: &ConsensusConfig, pow_window: Option<u64>) -> Value {
    match consensus {
        ConsensusConfig::Authority { key, slot_ms } => json!({
            "type": "authority",
            "key": key.to_string(),
            "slot_ms": slot_ms,
        }),
        ConsensusConfig::Rotating { slot_ms } => json!({ "type": "rotating", "slot_ms": slot_ms }),
        ConsensusConfig::Staked { slot_ms, schedule } => json!({ "type": "staked", "slot_ms": slot_ms, "schedule": schedule.name() }),
        ConsensusConfig::ProofOfWork(pow) => {
            let mut value = json!({
                "type": "pow",
                "hash": pow.hash.name(),
                "target_block_ms": pow.target_block_ms,
                "initial_difficulty": pow.initial_difficulty.to_string(),
                "min_difficulty": pow.min_difficulty.to_string(),
            });

            // A later addition: left out while unset, as files from before it leave it out.
            if let Some(window) = pow_window {
                value["window"] = window.into();
            }

            value
        }
    }
}

/// A consensus from JSON, as consensus_json writes it: the consensus and, for proof of work, its
/// window, which genesis keeps beside it.
pub fn consensus_from_json(value: &Value, path: &str) -> Result<(ConsensusConfig, Option<u64>), String> {
    let mut consensus = Fields::of(value, path)?;
    let parsed = match consensus.text("type")?.as_str() {
        "authority" => {
            let config = ConsensusConfig::Authority {
                key: consensus.parse("key")?,
                slot_ms: consensus.whole("slot_ms")?,
            };

            (config, None)
        }
        "pow" => {
            let hash = consensus.hash("hash")?;
            let config = ConsensusConfig::ProofOfWork(PowConfig {
                hash,
                target_block_ms: consensus.whole("target_block_ms")?,
                initial_difficulty: consensus.parse("initial_difficulty")?,
                min_difficulty: consensus.parse("min_difficulty")?,
            });
            let window = consensus.later_whole("window")?;

            (config, window)
        }
        "rotating" => {
            let config = ConsensusConfig::Rotating {
                slot_ms: consensus.whole("slot_ms")?,
            };

            (config, None)
        }
        "staked" => {
            let config = ConsensusConfig::Staked {
                slot_ms: consensus.whole("slot_ms")?,
                schedule: match consensus.text("schedule")?.as_str() {
                    "draw" => crate::consensus::Schedule::Draw,
                    "priority" => crate::consensus::Schedule::Priority,
                    other => return Err(format!("{path}.schedule: unknown schedule {other}; use draw or priority")),
                },
            };

            (config, None)
        }
        other => return Err(format!("{path}.type: unknown consensus {other}; use pow, authority, rotating or staked")),
    };
    consensus.done()?;

    Ok(parsed)
}

/// Governance as JSON: the shape genesis files and GET /governance use.
pub fn governance_json(config: &GovernanceConfig) -> Value {
    match *config {
        #[cfg(feature = "governance-electorate")]
        GovernanceConfig::Electorate { approval_epochs } => json!({ "type": "electorate", "approval_epochs": approval_epochs }),
        #[cfg(feature = "governance-council")]
        GovernanceConfig::Council { ref members, threshold, approval_epochs } => json!({
            "type": "council",
            "members": members.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "threshold": threshold,
            "approval_epochs": approval_epochs,
        }),
    }
}

fn governance_from_json(mut governance: Fields) -> Result<GovernanceConfig, String> {
    let config = match governance.text("type")?.as_str() {
        #[cfg(feature = "governance-electorate")]
        "electorate" => GovernanceConfig::Electorate {
            approval_epochs: governance.whole("approval_epochs")?,
        },
        #[cfg(feature = "governance-council")]
        "council" => GovernanceConfig::Council {
            members: governance
                .strings("members")?
                .iter()
                .map(|text| text.parse().map_err(|_| format!("governance.members: cannot read {text}")))
                .collect::<Result<Vec<_>, String>>()?,
            threshold: governance.small("threshold")?,
            approval_epochs: governance.whole("approval_epochs")?,
        },
        other => return Err(format!("governance.type: unknown governance {other}; use {}", GovernanceConfig::KINDS.join(" or "))),
    };
    governance.done()?;

    Ok(config)
}

/// The coin as JSON: the shape genesis files and GET /status use.
pub fn coin_json(coin: &Coin) -> Value {
    json!({ "symbol": coin.symbol, "decimals": coin.decimals })
}

/// Fees as JSON: the shape genesis files and GET /status use.
pub fn fees_json(fees: &FeeConfig) -> Value {
    let weights = |weighted: &Weighted| {
        json!({
            "per_tx": weighted.per_tx,
            "per_byte": weighted.per_byte,
            "per_code_byte": weighted.per_code_byte,
            "fuel_per_gas": weighted.fuel_per_gas,
        })
    };
    let mut schedule = match &fees.schedule {
        ScheduleConfig::Weighted(weighted) => weights(weighted),
        ScheduleConfig::Contract(scorer) => {
            let mut value = weights(&scorer.weights);
            value["contract"] = scorer.contract.to_string().into();
            value["fuel"] = scorer.fuel.into();

            // A later addition: left out while empty, as files from before it leave it out.
            if !scorer.kinds.is_empty() {
                value["kinds"] = scorer.kinds.clone().into();
            }

            value
        }
    };
    schedule["type"] = fees.schedule.name().into();

    json!({
        "block_gas_limit": fees.block_gas_limit,
        "burn_permille": fees.burn_permille,
        "schedule": schedule,
        "pricing": match &fees.pricing {
            PricingConfig::BaseFee(base_fee) => json!({
                "type": "base_fee",
                "initial": base_fee.initial.to_string(),
                "min": base_fee.min.to_string(),
                "elasticity": base_fee.elasticity,
                "change_denominator": base_fee.change_denominator,
            }),
            PricingConfig::Bid(bid) => json!({
                "type": "bid",
                "min_price": bid.min_price.to_string(),
            }),
        },
    })
}

/// Reads fees as fees_json writes them.
pub fn fees_from_json(value: &Value) -> Result<FeeConfig, String> {
    fees_from_fields(Fields::of(value, "fees")?)
}

fn fees_from_fields(mut fees: Fields) -> Result<FeeConfig, String> {
    let mut schedule = fees.object("schedule")?;
    let kind = schedule.text("type")?;
    let weights = Weighted {
        per_tx: schedule.whole("per_tx")?,
        per_byte: schedule.whole("per_byte")?,
        per_code_byte: schedule.whole("per_code_byte")?,
        fuel_per_gas: schedule.whole("fuel_per_gas")?,
    };
    let schedule_config = match kind.as_str() {
        "weighted" => ScheduleConfig::Weighted(weights),
        "contract" => ScheduleConfig::Contract(ContractScorer {
            weights,
            contract: schedule.parse("contract")?,
            fuel: schedule.whole("fuel")?,
            kinds: match schedule.map.contains_key("kinds") {
                true => schedule.strings("kinds")?,
                false => Vec::new(),
            },
        }),
        other => return Err(format!("fees.schedule.type: unknown schedule {other}; use weighted or contract")),
    };
    schedule.done()?;
    let mut pricing = fees.object("pricing")?;
    let pricing_config = match pricing.text("type")?.as_str() {
        "base_fee" => PricingConfig::BaseFee(BaseFee {
            initial: pricing.amount("initial")?,
            min: pricing.amount("min")?,
            elasticity: pricing.whole("elasticity")?,
            change_denominator: pricing.whole("change_denominator")?,
        }),
        "bid" => PricingConfig::Bid(Bid {
            min_price: pricing.amount("min_price")?,
        }),
        other => return Err(format!("fees.pricing.type: unknown pricing {other}; use base_fee or bid")),
    };
    pricing.done()?;
    let config = FeeConfig {
        block_gas_limit: fees.whole("block_gas_limit")?,
        burn_permille: u16::try_from(fees.whole("burn_permille")?).map_err(|_| "fees.burn_permille is too large".to_string())?,
        schedule: schedule_config,
        pricing: pricing_config,
    };
    fees.done()?;

    Ok(config)
}

/// Finality as JSON, its kind in "type" as consensus has it.
fn finality_json(config: &FinalityConfig) -> Value {
    match *config {
        #[cfg(feature = "finality-ffg")]
        FinalityConfig::Ffg(ref ffg) => {
            let mut value = json!({
            "type": "ffg",
            "checkpoint_interval": ffg.checkpoint_interval,
            "vote_depth": ffg.vote_depth,
            "min_bond": ffg.min_bond.to_string(),
            "activation_delay": ffg.activation_delay,
            "withdrawal_delay": ffg.withdrawal_delay,
            "reporter_divisor": ffg.reporter_divisor,
            "max_validators": ffg.max_validators,
            "vote_reward": ffg.vote_reward.to_string(),
            "max_proofs_per_block": ffg.max_proofs_per_block,
            "validators": ffg.validators.iter().map(|validator| json!({
                "account": validator.account.to_string(),
                "voter": validator.voter.to_string(),
                "bond": validator.bond.to_string(),
            })).collect::<Vec<_>>(),
            });

            // A later addition: left out while unset, as files from before it leave it out.
            if let Some(leak) = ffg.leak {
                value["leak"] = json!({ "after_epochs": leak.after_epochs, "permille": leak.permille });
            }

            value
        }
        #[cfg(feature = "finality-authority")]
        FinalityConfig::Authority(ref authority) => json!({
            "type": "authority",
            "checkpoint_interval": authority.checkpoint_interval,
            "vote_depth": authority.vote_depth,
            "activation_delay": authority.activation_delay,
            "max_signers": authority.max_signers,
            "signers": authority.signers.iter().map(|signer| json!({ "key": signer.key.to_string(), "account": signer.account.to_string() })).collect::<Vec<_>>(),
            "threshold": authority.threshold,
            "change_threshold": authority.change_threshold,
            "approval_epochs": authority.approval_epochs,
            "vote_reward": authority.vote_reward.to_string(),
            "max_proofs_per_block": authority.max_proofs_per_block,
        }),
        #[cfg(feature = "finality-depth")]
        FinalityConfig::Depth(ref depth) => json!({
            "type": "depth",
            "depth": depth.depth,
        }),
    }
}

/// Reads finality, whichever kind its "type" names.
#[cfg_attr(
    not(any(feature = "finality-ffg", feature = "finality-authority", feature = "finality-depth")),
    allow(unreachable_code, unused_variables)
)]
fn finality_from_json(mut finality: Fields) -> Result<FinalityConfig, String> {
    let config = match finality.text("type")?.as_str() {
        #[cfg(feature = "finality-ffg")]
        "ffg" => FinalityConfig::Ffg(FfgConfig {
            checkpoint_interval: finality.whole("checkpoint_interval")?,
            vote_depth: finality.whole("vote_depth")?,
            min_bond: finality.amount("min_bond")?,
            activation_delay: finality.whole("activation_delay")?,
            withdrawal_delay: finality.whole("withdrawal_delay")?,
            reporter_divisor: finality.whole("reporter_divisor")?,
            max_validators: finality.small("max_validators")?,
            vote_reward: finality.amount("vote_reward")?,
            max_proofs_per_block: finality.small("max_proofs_per_block")?,
            validators: finality
                .list("validators")?
                .into_iter()
                .map(|mut item| {
                    let validator = GenesisValidator {
                        account: item.parse("account")?,
                        voter: item.parse::<PublicKey>("voter")?,
                        bond: item.amount("bond")?,
                    };
                    item.done()?;

                    Ok(validator)
                })
                .collect::<Result<Vec<_>, String>>()?,
            leak: match finality.map.contains_key("leak") {
                true => match finality.optional_object("leak")? {
                    Some(mut leak) => {
                        let value = crate::chain::Leak {
                            after_epochs: leak.whole("after_epochs")?,
                            permille: leak.whole("permille")?,
                        };
                        leak.done()?;

                        Some(value)
                    }
                    None => None,
                },
                false => None,
            },
        }),
        #[cfg(feature = "finality-authority")]
        "authority" => FinalityConfig::Authority(AuthorityConfig {
            checkpoint_interval: finality.whole("checkpoint_interval")?,
            vote_depth: finality.whole("vote_depth")?,
            activation_delay: finality.whole("activation_delay")?,
            max_signers: finality.small("max_signers")?,
            signers: finality.signers("signers")?,
            threshold: finality.small("threshold")?,
            change_threshold: finality.small("change_threshold")?,
            approval_epochs: finality.whole("approval_epochs")?,
            vote_reward: finality.amount("vote_reward")?,
            max_proofs_per_block: finality.small("max_proofs_per_block")?,
        }),
        #[cfg(feature = "finality-depth")]
        "depth" => FinalityConfig::Depth(DepthConfig {
            depth: finality.whole("depth")?,
        }),
        other => return Err(format!("finality.type: unknown finality {other}; {}", kinds())),
    };
    finality.done()?;

    Ok(config)
}

pub fn from_json(value: &Value) -> Result<Genesis, String> {
    let mut root = Fields::of(value, "genesis")?;
    let mut coin = root.object("coin")?;
    let coin_value = Coin {
        symbol: coin.text("symbol")?,
        decimals: u8::try_from(coin.whole("decimals")?).map_err(|_| "coin.decimals is too large".to_string())?,
    };
    coin.done()?;
    let (consensus, pow_window) = consensus_from_json(root.get("consensus")?, "consensus")?;
    let mut reward = root.object("reward")?;
    let reward_value = Reward {
        initial: reward.amount("initial")?,
        halving_interval: reward.optional_whole("halving_interval")?,
    };
    reward.done()?;
    let minter = root.optional_parse("minter")?;
    let allocations = root
        .list("allocations")?
        .into_iter()
        .map(|mut item| {
            let allocation = Allocation {
                address: item.parse("address")?,
                amount: item.amount("amount")?,
            };
            item.done()?;

            Ok(allocation)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let engines = root
        .strings("engines")?
        .iter()
        .map(|name| match name.as_str() {
            "lua" => Ok(Engine::Lua),
            "wasm" => Ok(Engine::Wasm),
            other => Err(format!("engines: unknown engine {other}; use lua or wasm")),
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut limits = root.object("tx_limits")?;
    let tx_limits = Limits {
        fuel: limits.whole("fuel")?,
        memory: usize::try_from(limits.whole("memory")?).map_err(|_| "tx_limits.memory is too large".to_string())?,
    };
    limits.done()?;
    let token_functions = root.strings("token_functions")?;
    let max_reorg_depth = root.whole("max_reorg_depth")?;
    let finality = root.optional_object("finality")?.map(finality_from_json).transpose()?;
    let fees = root.optional_object("fees")?.map(fees_from_fields).transpose()?;
    // Absent from files written before it: those chains keep their id, bounded by gas alone.
    let max_block_bytes = root.later_whole("max_block_bytes")?;
    let vote_reward_source = match root.map.contains_key("vote_reward_source") {
        true => root.optional_parse("vote_reward_source")?,
        false => None,
    };
    // Absent from files written before it: those chains keep their id, and nothing changes them.
    let governance = match root.map.contains_key("governance") {
        true => root.optional_object("governance")?.map(governance_from_json).transpose()?,
        false => None,
    };
    let genesis = Genesis {
        timestamp: root.whole("timestamp")?,
        hash: root.hash("hash")?,
        coin: coin_value,
        consensus,
        reward: reward_value,
        minter,
        allocations,
        engines,
        tx_limits,
        token_functions,
        max_reorg_depth,
        finality,
        fees,
        max_block_bytes,
        pow_window,
        governance,
        vote_reward_source,
    };
    root.done()?;

    Ok(genesis)
}

/// The fields of one JSON object, tracking which were read so leftovers can be refused.
struct Fields<'a> {
    path: String,
    map: &'a Map<String, Value>,
    used: HashSet<&'a str>,
}

impl<'a> Fields<'a> {
    fn of(value: &'a Value, path: &str) -> Result<Fields<'a>, String> {
        let map = value.as_object().ok_or_else(|| format!("{path} must be an object"))?;

        Ok(Fields {
            path: path.to_string(),
            map,
            used: HashSet::new(),
        })
    }

    fn name(&self, key: &str) -> String {
        match self.path.as_str() {
            "genesis" => key.to_string(),
            path => format!("{path}.{key}"),
        }
    }

    fn get(&mut self, key: &'a str) -> Result<&'a Value, String> {
        self.used.insert(key);
        self.map.get(key).ok_or_else(|| format!("{} is missing", self.name(key)))
    }

    fn object(&mut self, key: &'a str) -> Result<Fields<'a>, String> {
        let name = self.name(key);
        Fields::of(self.get(key)?, &name)
    }

    fn optional_object(&mut self, key: &'a str) -> Result<Option<Fields<'a>>, String> {
        let name = self.name(key);

        match self.get(key)? {
            Value::Null => Ok(None),
            value => Fields::of(value, &name).map(Some),
        }
    }

    fn list(&mut self, key: &'a str) -> Result<Vec<Fields<'a>>, String> {
        let name = self.name(key);
        let items = self.get(key)?.as_array().ok_or_else(|| format!("{name} must be a list"))?;

        items
            .iter()
            .enumerate()
            .map(|(index, item)| Fields::of(item, &format!("{name}[{index}]")))
            .collect()
    }

    /// Authority signers: each a key as a string, its own account, or an object with a key and an
    /// account.
    #[cfg_attr(not(feature = "finality-authority"), allow(dead_code))]
    fn signers(&mut self, key: &'a str) -> Result<Vec<crate::registry::signers::Signer>, String> {
        use crate::registry::signers::Signer;

        let name = self.name(key);
        let items = self.get(key)?.as_array().ok_or_else(|| format!("{name} must be a list"))?;

        items
            .iter()
            .enumerate()
            .map(|(index, item)| match item {
                Value::String(text) => text
                    .parse::<crate::crypto::keypair::PublicKey>()
                    .map(Signer::own)
                    .map_err(|_| format!("{name}[{index}]: cannot read {text}")),
                Value::Object(_) => {
                    let mut fields = Fields::of(item, &format!("{name}[{index}]"))?;
                    let signer = Signer {
                        key: fields.parse("key")?,
                        account: fields.parse("account")?,
                    };
                    fields.done()?;

                    Ok(signer)
                }
                _ => Err(format!("{name}[{index}] must be a key or an object with a key and an account")),
            })
            .collect()
    }

    fn strings(&mut self, key: &'a str) -> Result<Vec<String>, String> {
        let name = self.name(key);
        let items = self.get(key)?.as_array().ok_or_else(|| format!("{name} must be a list"))?;

        items
            .iter()
            .map(|item| item.as_str().map(str::to_string).ok_or_else(|| format!("{name} must hold strings")))
            .collect()
    }

    fn text(&mut self, key: &'a str) -> Result<String, String> {
        let name = self.name(key);

        self.get(key)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{name} must be a string"))
    }

    fn parse<T: std::str::FromStr>(&mut self, key: &'a str) -> Result<T, String> {
        let name = self.name(key);
        let text = self.text(key)?;

        text.parse().map_err(|_| format!("{name}: cannot read {text}"))
    }

    fn optional_parse<T: std::str::FromStr>(&mut self, key: &'a str) -> Result<Option<T>, String> {
        match self.get(key)? {
            Value::Null => Ok(None),
            _ => self.parse(key).map(Some),
        }
    }

    fn whole(&mut self, key: &'a str) -> Result<u64, String> {
        let name = self.name(key);

        self.get(key)?
            .as_u64()
            .ok_or_else(|| format!("{name} must be a whole number"))
    }

    /// A whole number that fits 32 bits.
    #[cfg_attr(not(any(feature = "finality-ffg", feature = "finality-authority")), allow(dead_code))]
    fn small(&mut self, key: &'a str) -> Result<u32, String> {
        let name = self.name(key);

        u32::try_from(self.whole(key)?).map_err(|_| format!("{name} is too large"))
    }

    fn optional_whole(&mut self, key: &'a str) -> Result<Option<u64>, String> {
        match self.get(key)? {
            Value::Null => Ok(None),
            _ => self.whole(key).map(Some),
        }
    }

    /// A field added after genesis files were first written: one without it means None.
    fn later_whole(&mut self, key: &'a str) -> Result<Option<u64>, String> {
        match self.map.contains_key(key) {
            true => self.optional_whole(key),
            false => Ok(None),
        }
    }

    /// A hash algorithm by name, such as blake3 or sha256.
    fn hash(&mut self, key: &'a str) -> Result<HashType, String> {
        let name = self.name(key);

        self.text(key)?.parse().map_err(|err| format!("{name}: {err}"))
    }

    /// An amount, written as a decimal string so it can pass 2^53.
    fn amount(&mut self, key: &'a str) -> Result<u256, String> {
        let name = self.name(key);
        let text = self.text(key)?;

        u256::from_str_radix(&text, 10).map_err(|_| format!("{name}: {text} is not a whole amount"))
    }

    fn done(self) -> Result<(), String> {
        let mut unknown: Vec<&String> = self.map.keys().filter(|key| !self.used.contains(key.as_str())).collect();
        unknown.sort();

        match unknown.first() {
            Some(key) => Err(format!("{} is not a genesis field", self.name(key))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        empty.governance = Some(GovernanceConfig::Council { members: Vec::new(), threshold: 1, approval_epochs: 64 });

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
        // from: its hash, key, engines and finality all go into it.
        if cfg!(all(
            feature = "blake3",
            feature = "ed25519",
            feature = "lvm",
            feature = "wasm",
            feature = "finality-ffg",
            feature = "finality-authority",
            feature = "finality-depth"
        )) {
            assert_eq!(read.block().id().to_string(), "0xf55e42d5e8776e763713812686978872028563a6640412515a05d6d64043b181");
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
            ("/engines", json!(["lua", "evm"]), "unknown engine evm"),
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
}
