//! Genesis files: a chain's whole configuration as JSON, so every parameter can be chosen without
//! touching code. `chainv4 genesis` writes one with the development defaults; `chainv4 --genesis
//! FILE` runs a node on it. Every field must be present, and unknown fields are refused, so a
//! misspelled parameter is an error rather than a silent default.

#[cfg(feature = "finality-authority")]
use crate::chain::AuthorityConfig;
#[cfg(feature = "finality-depth")]
use crate::chain::DepthConfig;
#[cfg(feature = "finality-ffg")]
use crate::chain::{FfgConfig, GenesisValidator};
#[cfg(any(feature = "finality-ffg", feature = "finality-authority"))]
use crate::crypto::keypair::PublicKey;
use {
    super::CliResult,
    crate::chain::fees::{BaseFee, Bid, ContractScorer, PricingConfig, ScheduleConfig, Weighted},
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
    /// Where to write it. Without one, it goes to stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum DevChain {
    Pow,
    Authority,
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

/// Development fees of a kind, with a scorer contract if one is named.
pub fn dev_fees(kind: FeeKind, scorer: Option<Address>) -> Option<FeeConfig> {
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

    genesis.fees = dev_fees(args.fees, args.scorer);

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
        "consensus": match &genesis.consensus {
            ConsensusConfig::Authority { key, slot_ms } => json!({
                "type": "authority",
                "key": key.to_string(),
                "slot_ms": slot_ms,
            }),
            ConsensusConfig::ProofOfWork(pow) => json!({
                "type": "pow",
                "hash": pow.hash.name(),
                "target_block_ms": pow.target_block_ms,
                "initial_difficulty": pow.initial_difficulty.to_string(),
                "min_difficulty": pow.min_difficulty.to_string(),
            }),
        },
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
    })
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
        FinalityConfig::Ffg(ref ffg) => json!({
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
        }),
        #[cfg(feature = "finality-authority")]
        FinalityConfig::Authority(ref authority) => json!({
            "type": "authority",
            "checkpoint_interval": authority.checkpoint_interval,
            "vote_depth": authority.vote_depth,
            "activation_delay": authority.activation_delay,
            "max_signers": authority.max_signers,
            "signers": authority.signers.iter().map(|signer| signer.to_string()).collect::<Vec<_>>(),
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
        }),
        #[cfg(feature = "finality-authority")]
        "authority" => FinalityConfig::Authority(AuthorityConfig {
            checkpoint_interval: finality.whole("checkpoint_interval")?,
            vote_depth: finality.whole("vote_depth")?,
            activation_delay: finality.whole("activation_delay")?,
            max_signers: finality.small("max_signers")?,
            signers: finality
                .strings("signers")?
                .iter()
                .map(|text| text.parse::<PublicKey>().map_err(|_| format!("finality.signers: cannot read {text}")))
                .collect::<Result<Vec<_>, String>>()?,
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
    let mut consensus = root.object("consensus")?;
    let consensus = match consensus.text("type")?.as_str() {
        "authority" => {
            let config = ConsensusConfig::Authority {
                key: consensus.parse("key")?,
                slot_ms: consensus.whole("slot_ms")?,
            };
            consensus.done()?;

            config
        }
        "pow" => {
            let hash = consensus.hash("hash")?;
            let config = ConsensusConfig::ProofOfWork(PowConfig {
                hash,
                target_block_ms: consensus.whole("target_block_ms")?,
                initial_difficulty: consensus.parse("initial_difficulty")?,
                min_difficulty: consensus.parse("min_difficulty")?,
            });
            consensus.done()?;

            config
        }
        other => return Err(format!("consensus.type: unknown consensus {other}; use pow or authority")),
    };
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

    #[test]
    fn genesis_files_round_trip_exactly() {
        let scorer = Address::Contract([5; 32]);
        let fee_kinds = [
            (FeeKind::BaseFee, None),
            (FeeKind::Bid, None),
            (FeeKind::BaseFee, Some(scorer)),
            (FeeKind::Free, None),
        ];

        for chain in [DevChain::Pow, DevChain::Authority] {
            for &kind in ALL_FINALITY {
                for (fees, scorer) in fee_kinds {
                    let mut genesis = dev(chain);
                    genesis.finality = dev_finality(kind);
                    genesis.fees = dev_fees(fees, scorer);
                    let read = from_json(&to_json(&genesis)).unwrap();

                    assert_eq!(read, genesis);
                    assert_eq!(read.block().id(), genesis.block().id());
                }
            }
        }
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
            let signers: Vec<PublicKey> = (0..3)
                .map(|_| crate::crypto::keypair::Keypair::generate(scheme).unwrap().public())
                .collect();
            let mut value = with(FinalityKind::Authority);
            value["finality"]["signers"] = json!(signers.iter().map(PublicKey::to_string).collect::<Vec<_>>());
            value["finality"]["threshold"] = json!(2);
            let Some(FinalityConfig::Authority(authority)) = from_json(&value).unwrap().finality else {
                panic!("not authority")
            };

            assert_eq!((authority.signers, authority.threshold), (signers, 2));
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
