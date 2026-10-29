//! A change under governance as JSON, and each part of genesis one may replace: consensus, fees,
//! the reward, limits, finality, the federation, the directory, the collection standards and name
//! prices, as genesis files write them. Plain data, as params is, so the apps read and write a
//! change as the node does; genesis files themselves are chain::json's. Every field must be
//! present, and unknown fields are refused, so a misspelled parameter is an error rather than a
//! silent default.

#[cfg(any(feature = "finality-authority", not(feature = "node")))]
use crate::params::finality::AuthorityConfig;
#[cfg(any(feature = "finality-depth", not(feature = "node")))]
use crate::params::finality::DepthConfig;
#[cfg(any(feature = "finality-ffg", not(feature = "node")))]
use {crate::crypto::keypair::PublicKey, crate::params::finality::{FfgConfig, GenesisValidator}};
use {
    crate::consensus::{ConsensusConfig, PowConfig},
    crate::params::fees::{BaseFee, Bid, ContractScorer, FeeConfig, PricingConfig, ScheduleConfig, Weighted},
    crate::params::finality::FinalityConfig,
    crate::params::{Limits, Reward},
    crate::registry::collections::CollectionFunctions,
    crate::registry::directory::{Directory, EvmGateway},
    crate::registry::federation::Federation,
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::u256::u256,
    serde_json::{Map, Value, json},
    std::collections::HashSet,
};

/// A federation as JSON: the shape genesis files, GET /status and approve-change use, keys as 0x
/// and 66 hex digits.
pub fn federation_json(federation: &Federation) -> Value {
    json!({
        "signers": federation.signers.iter().map(|key| crate::structs::hex::to_hex(key)).collect::<Vec<_>>(),
        "threshold": federation.threshold,
        "chain_depth": federation.chain_depth,
        "evm_depth": federation.evm_depth,
        "bitcoin_depth": federation.bitcoin_depth,
    })
}

pub(crate) fn federation_from_json(mut fields: Fields) -> Result<Federation, String> {
    let signers = fields
        .strings("signers")?
        .iter()
        .map(|text| {
            crate::structs::hex::from_hex(text)
                .and_then(|bytes| <[u8; 33]>::try_from(bytes).ok())
                .ok_or_else(|| format!("federation.signers: {text} is not a compressed key, 0x and 66 hex digits"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let federation = Federation {
        signers,
        threshold: u32::try_from(fields.whole("threshold")?).map_err(|_| "federation.threshold is too large".to_string())?,
        chain_depth: fields.whole("chain_depth")?,
        evm_depth: fields.whole("evm_depth")?,
        bitcoin_depth: u32::try_from(fields.whole("bitcoin_depth")?).map_err(|_| "federation.bitcoin_depth is too large".to_string())?,
    };
    fields.done()?;

    Ok(federation)
}

/// The directory as JSON: the shape genesis files, GET /status and approve-change use. Contracts
/// here as addresses, EVM ones as 0x and 40 hex digits, each null while there is none.
pub fn directory_json(directory: &Directory) -> Value {
    let here = |address: Option<Address>| address.map(|address| address.to_string());

    json!({
        "gateway": here(directory.gateway),
        "btc_gateway": here(directory.btc_gateway),
        "evm_gateways": directory.evm_gateways.iter().map(|evm| json!({ "chain_id": evm.chain_id, "gateway": crate::structs::hex::to_hex(&evm.gateway) })).collect::<Vec<_>>(),
        "swap": here(directory.swap),
        "dex": here(directory.dex),
        "dex_router": directory.dex_router.map(|router| crate::structs::hex::to_hex(&router)),
        "nft_factory": here(directory.nft_factory),
    })
}

pub(crate) fn evm_address(text: &str, what: &str) -> Result<[u8; 20], String> {
    crate::structs::hex::from_hex(text)
        .and_then(|bytes| <[u8; 20]>::try_from(bytes).ok())
        .ok_or_else(|| format!("{what}: {text} is not an EVM address, 0x and 40 hex digits"))
}

pub(crate) fn directory_from_json(mut fields: Fields) -> Result<Directory, String> {
    let gateway = fields.optional_parse("gateway")?;
    let btc_gateway = fields.optional_parse("btc_gateway")?;
    let evm_gateways = fields
        .list("evm_gateways")?
        .into_iter()
        .map(|mut item| {
            let chain_id = item.whole("chain_id")?;
            let gateway = evm_address(&item.text("gateway")?, "directory.evm_gateways")?;
            item.done()?;

            Ok(EvmGateway { chain_id, gateway })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let swap = fields.optional_parse("swap")?;
    let dex = fields.optional_parse("dex")?;
    let dex_router = match fields.get("dex_router")? {
        Value::Null => None,
        _ => Some(evm_address(&fields.text("dex_router")?, "directory.dex_router")?),
    };
    let nft_factory = fields.optional_parse("nft_factory")?;
    let directory = Directory { gateway, btc_gateway, evm_gateways, swap, dex, dex_router, nft_factory };
    fields.done()?;

    Ok(directory)
}

/// A directory from approve-change's --directory: the same JSON as a genesis file's.
pub fn directory_from_text(text: &str) -> Result<Directory, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let directory = directory_from_json(Fields::of(&value, "directory")?)?;

    match directory.problem() {
        Some(problem) => Err(problem.to_string()),
        None => Ok(directory),
    }
}

/// A federation from approve-change's --federation: the same JSON as a genesis file's.
pub fn federation_from_text(text: &str) -> Result<Federation, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let federation = federation_from_json(Fields::of(&value, "federation")?)?;

    match federation.problem() {
        Some(problem) => Err(problem.to_string()),
        None => Ok(federation),
    }
}

/// What makes a collection as JSON: the shape genesis files, GET /status and approve-change use.
pub fn collections_json(functions: &CollectionFunctions) -> Value {
    json!({ "nft": functions.nft, "multi": functions.multi })
}

pub(crate) fn collections_from_json(mut fields: Fields) -> Result<CollectionFunctions, String> {
    let functions = CollectionFunctions {
        nft: fields.strings("nft")?,
        multi: fields.strings("multi")?,
    };
    fields.done()?;

    Ok(functions)
}

/// What makes a collection, from approve-change's --collections: standard, for the standards' own
/// functions, or JSON with nft and multi lists.
pub fn collections_from_text(text: &str) -> Result<CollectionFunctions, String> {
    let functions = match text.trim() {
        "standard" => CollectionFunctions::standard(),
        text => {
            let value: Value = serde_json::from_str(text).map_err(|err| format!("not standard, nor JSON: {err}"))?;

            collections_from_json(Fields::of(&value, "collections")?)?
        }
    };

    match functions.problem() {
        Some(problem) => Err(problem.to_string()),
        None => Ok(functions),
    }
}

/// Prices as a list of decimal strings, as genesis files and approve-change write them.
pub fn prices_from_json(value: &Value, name: &str) -> Result<Vec<u256>, String> {
    let items = value.as_array().ok_or_else(|| format!("{name} must be a list of amounts"))?;

    items
        .iter()
        .map(|item| match item {
            Value::String(text) => u256::from_str_radix(text, 10).map_err(|_| format!("{name}: {text} is not a whole amount")),
            _ => Err(format!("{name} holds amounts as strings, such as \"5000000000000000000\"")),
        })
        .collect()
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
/// Fees from approve-change's --fees: a genesis file's fees object, checked as genesis is.
pub fn fees_from_text(text: &str) -> Result<FeeConfig, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let fees = fees_from_json(&value)?;

    fees.problem().map_or(Ok(fees), |problem| Err(problem.to_string()))
}

/// A block reward as JSON: the shape genesis files and approve-change use.
pub fn reward_json(reward: &Reward) -> Value {
    json!({ "initial": reward.initial.to_string(), "halving_interval": reward.halving_interval })
}

/// A reward from approve-change's --reward.
pub fn reward_from_text(text: &str) -> Result<Reward, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let mut fields = Fields::of(&value, "reward")?;
    let reward = Reward { initial: fields.amount("initial")?, halving_interval: fields.optional_whole("halving_interval")? };
    fields.done()?;

    crate::params::reward_problem(&reward).map_or(Ok(reward), |problem| Err(problem.to_string()))
}

/// Limits as JSON: genesis's tx_limits and max_block_bytes, together.
pub fn limits_json(limits: &crate::params::BlockLimits) -> Value {
    json!({ "tx_limits": { "fuel": limits.tx_limits.fuel, "memory": limits.tx_limits.memory }, "max_block_bytes": limits.max_block_bytes })
}

/// Limits from approve-change's --limits.
pub fn limits_from_text(text: &str) -> Result<crate::params::BlockLimits, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;
    let mut fields = Fields::of(&value, "limits")?;
    let mut tx = fields.object("tx_limits")?;
    let tx_limits = Limits { fuel: tx.whole("fuel")?, memory: usize::try_from(tx.whole("memory")?).map_err(|_| "limits.tx_limits.memory is too large".to_string())? };
    tx.done()?;
    let limits = crate::params::BlockLimits { tx_limits, max_block_bytes: fields.optional_whole("max_block_bytes")? };
    fields.done()?;

    limits.problem().map_or(Ok(limits), |problem| Err(problem.to_string()))
}

/// Finality from approve-change's --finality: a genesis file's finality object. Whether it only
/// changes what may change is the chain's to check, against its genesis.
pub fn finality_from_text(text: &str) -> Result<FinalityConfig, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| format!("not JSON: {err}"))?;

    finality_from_json(Fields::of(&value, "finality")?)
}

pub fn fees_from_json(value: &Value) -> Result<FeeConfig, String> {
    fees_from_fields(Fields::of(value, "fees")?)
}

pub(crate) fn fees_from_fields(mut fees: Fields) -> Result<FeeConfig, String> {
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
pub fn finality_json(config: &FinalityConfig) -> Value {
    match *config {
        #[cfg(any(feature = "finality-ffg", not(feature = "node")))]
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
        #[cfg(any(feature = "finality-authority", not(feature = "node")))]
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
        #[cfg(any(feature = "finality-depth", not(feature = "node")))]
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
pub(crate) fn finality_from_json(mut finality: Fields) -> Result<FinalityConfig, String> {
    let config = match finality.text("type")?.as_str() {
        #[cfg(any(feature = "finality-ffg", not(feature = "node")))]
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
                        let value = crate::registry::validators::Leak {
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
        #[cfg(any(feature = "finality-authority", not(feature = "node")))]
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
        #[cfg(any(feature = "finality-depth", not(feature = "node")))]
        "depth" => FinalityConfig::Depth(DepthConfig {
            depth: finality.whole("depth")?,
        }),
        other => return Err(format!("finality.type: unknown finality {other}; {}", finality_kinds())),
    };
    finality.done()?;

    Ok(config)
}

/// The fields of one JSON object, tracking which were read so leftovers can be refused.
pub(crate) struct Fields<'a> {
    path: String,
    pub(crate) map: &'a Map<String, Value>,
    used: HashSet<&'a str>,
}

// Some are for genesis files alone, which only a node reads.
#[cfg_attr(not(feature = "node"), allow(dead_code))]
impl<'a> Fields<'a> {
    pub(crate) fn of(value: &'a Value, path: &str) -> Result<Fields<'a>, String> {
        let map = value.as_object().ok_or_else(|| format!("{path} must be an object"))?;

        Ok(Fields {
            path: path.to_string(),
            map,
            used: HashSet::new(),
        })
    }

    pub(crate) fn name(&self, key: &str) -> String {
        match self.path.as_str() {
            "genesis" => key.to_string(),
            path => format!("{path}.{key}"),
        }
    }

    pub(crate) fn get(&mut self, key: &'a str) -> Result<&'a Value, String> {
        self.used.insert(key);
        self.map.get(key).ok_or_else(|| format!("{} is missing", self.name(key)))
    }

    pub(crate) fn object(&mut self, key: &'a str) -> Result<Fields<'a>, String> {
        let name = self.name(key);
        Fields::of(self.get(key)?, &name)
    }

    pub(crate) fn optional_object(&mut self, key: &'a str) -> Result<Option<Fields<'a>>, String> {
        let name = self.name(key);

        match self.get(key)? {
            Value::Null => Ok(None),
            value => Fields::of(value, &name).map(Some),
        }
    }

    pub(crate) fn list(&mut self, key: &'a str) -> Result<Vec<Fields<'a>>, String> {
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
    pub(crate) fn signers(&mut self, key: &'a str) -> Result<Vec<crate::registry::signers::Signer>, String> {
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

    pub(crate) fn strings(&mut self, key: &'a str) -> Result<Vec<String>, String> {
        let name = self.name(key);
        let items = self.get(key)?.as_array().ok_or_else(|| format!("{name} must be a list"))?;

        items
            .iter()
            .map(|item| item.as_str().map(str::to_string).ok_or_else(|| format!("{name} must hold strings")))
            .collect()
    }

    pub(crate) fn text(&mut self, key: &'a str) -> Result<String, String> {
        let name = self.name(key);

        self.get(key)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("{name} must be a string"))
    }

    pub(crate) fn parse<T: std::str::FromStr>(&mut self, key: &'a str) -> Result<T, String> {
        let name = self.name(key);
        let text = self.text(key)?;

        text.parse().map_err(|_| format!("{name}: cannot read {text}"))
    }

    pub(crate) fn optional_text(&mut self, key: &'a str) -> Result<Option<String>, String> {
        match self.get(key)? {
            Value::Null => Ok(None),
            _ => self.text(key).map(Some),
        }
    }

    pub(crate) fn optional_parse<T: std::str::FromStr>(&mut self, key: &'a str) -> Result<Option<T>, String> {
        match self.get(key)? {
            Value::Null => Ok(None),
            _ => self.parse(key).map(Some),
        }
    }

    pub(crate) fn whole(&mut self, key: &'a str) -> Result<u64, String> {
        let name = self.name(key);

        self.get(key)?
            .as_u64()
            .ok_or_else(|| format!("{name} must be a whole number"))
    }

    /// A whole number that fits 32 bits.
    pub(crate) fn small(&mut self, key: &'a str) -> Result<u32, String> {
        let name = self.name(key);

        u32::try_from(self.whole(key)?).map_err(|_| format!("{name} is too large"))
    }

    pub(crate) fn optional_whole(&mut self, key: &'a str) -> Result<Option<u64>, String> {
        match self.get(key)? {
            Value::Null => Ok(None),
            _ => self.whole(key).map(Some),
        }
    }

    /// A field added after genesis files were first written: one without it means None.
    pub(crate) fn later_whole(&mut self, key: &'a str) -> Result<Option<u64>, String> {
        match self.map.contains_key(key) {
            true => self.optional_whole(key),
            false => Ok(None),
        }
    }

    /// A hash algorithm by name, such as blake3 or sha256.
    pub(crate) fn hash(&mut self, key: &'a str) -> Result<HashType, String> {
        let name = self.name(key);

        self.text(key)?.parse().map_err(|err| format!("{name}: {err}"))
    }

    /// An amount, written as a decimal string so it can pass 2^53.
    pub(crate) fn amount(&mut self, key: &'a str) -> Result<u256, String> {
        let name = self.name(key);
        let text = self.text(key)?;

        u256::from_str_radix(&text, 10).map_err(|_| format!("{name}: {text} is not a whole amount"))
    }

    pub(crate) fn done(self) -> Result<(), String> {
        let mut unknown: Vec<&String> = self.map.keys().filter(|key| !self.used.contains(key.as_str())).collect();
        unknown.sort();

        match unknown.first() {
            Some(key) => Err(format!("{} is not a genesis field", self.name(key))),
            None => Ok(()),
        }
    }
}

/// The finality kinds this build has, for error messages.
pub fn finality_kinds() -> String {
    match FinalityConfig::KINDS {
        [] => "this build has none".to_string(),
        kinds => format!("this build has {}", kinds.join(", ")),
    }
}

/// A change under governance as JSON: one field, named for what it changes, holding the new value
/// as genesis files write it. What GET /governance shows and change_from_json reads back.
pub fn change_json(change: &crate::governance::Change) -> Value {
    use crate::governance::Change;

    match change {
        Change::Consensus(consensus) => json!({ "consensus": consensus_json(consensus, None) }),
        Change::Bootnodes(bootnodes) => json!({ "bootnodes": bootnodes }),
        Change::NamePrices(prices) => json!({ "name_prices": prices.iter().map(u256::to_string).collect::<Vec<_>>() }),
        Change::Collections(functions) => json!({ "collections": collections_json(functions) }),
        Change::Federation(federation) => json!({ "federation": federation_json(federation) }),
        Change::Directory(directory) => json!({ "directory": directory_json(directory) }),
        Change::Fees(fees) => json!({ "fees": fees_json(fees) }),
        Change::Reward(reward) => json!({ "reward": reward_json(reward) }),
        Change::Limits(limits) => json!({ "limits": limits_json(limits) }),
        Change::Finality(finality) => json!({ "finality": finality_json(finality) }),
        Change::SwitchFinality { epoch, config } => json!({ "switch_finality": { "epoch": epoch, "finality": finality_json(config) } }),
        Change::Upgrade(upgrade) => json!({ "upgrade": { "least": upgrade.least.to_string(), "from": upgrade.from } }),
        Change::Name(name) => json!({ "name": name }),
    }
}

/// A change under governance from its JSON, as change_json writes it, checked as the CLI's
/// approve-change checks its flags: what running it checks against the chain, such as whether a
/// consensus fits its finality, waits for the transaction.
pub fn change_from_json(value: &Value) -> Result<crate::governance::Change, String> {
    use crate::governance::Change;

    let object = value.as_object().ok_or("a change must be an object")?;
    let [(kind, value)] = object.iter().collect::<Vec<_>>()[..] else {
        return Err("a change has one field, named for what it changes".to_string());
    };
    let checked = |problem: Option<&str>| problem.map_or(Ok(()), |problem| Err(format!("{kind}: {problem}")));

    let change = match kind.as_str() {
        "consensus" => match consensus_from_json(value, "consensus")? {
            (consensus, None) => Change::Consensus(consensus),
            (_, Some(_)) => return Err("consensus: a change keeps the chain's window; leave it out".to_string()),
        },
        "bootnodes" => {
            let bootnodes = value
                .as_array()
                .ok_or("bootnodes must be a list")?
                .iter()
                .map(|address| address.as_str().map(|address| address.trim().to_string()).ok_or("bootnodes must be strings"))
                .collect::<Result<Vec<_>, _>>()?;
            checked(crate::governance::bootnodes_problem(&bootnodes))?;

            Change::Bootnodes(bootnodes)
        }
        "name_prices" => {
            let prices = prices_from_json(value, "name_prices")?;
            checked(crate::registry::names::prices_problem(&prices))?;

            Change::NamePrices(prices)
        }
        "collections" => match value.as_str() {
            Some(text) => Change::Collections(collections_from_text(text)?),
            None => Change::Collections(collections_from_text(&value.to_string())?),
        },
        "federation" => Change::Federation(federation_from_text(&value.to_string())?),
        "directory" => Change::Directory(directory_from_text(&value.to_string())?),
        "fees" => Change::Fees(fees_from_text(&value.to_string())?),
        "reward" => Change::Reward(reward_from_text(&value.to_string())?),
        "limits" => Change::Limits(limits_from_text(&value.to_string())?),
        "finality" => Change::Finality(finality_from_text(&value.to_string())?),
        "switch_finality" => {
            let mut fields = Fields::of(value, "switch_finality")?;
            let epoch = fields.whole("epoch")?;
            let config = finality_from_text(&fields.get("finality")?.to_string())?;
            fields.done()?;

            Change::SwitchFinality { epoch, config }
        }
        "upgrade" => {
            let mut fields = Fields::of(value, "upgrade")?;
            let least = fields.text("least")?.parse().map_err(|why| format!("upgrade.least: {why}"))?;
            let from = fields.whole("from")?;
            fields.done()?;

            Change::Upgrade(crate::structs::version::Upgrade { least, from })
        }
        "name" => {
            let name = value.as_str().ok_or("name must be a string")?.to_string();
            checked(crate::governance::chain_name_problem(&name))?;

            Change::Name(name)
        }
        other => return Err(format!("{other} is not something governance changes")),
    };

    Ok(change)
}

/// Part of a setting, laid over the whole of it as it stands: each field given replaces the one
/// standing, an object's fields one by one, and anything else whole. What makes a change that
/// names only some fields, such as fees' burn_permille alone, into the whole value a change
/// carries; the kind's version keeps it from undoing a decision made in between.
pub fn over(standing: &Value, given: &Value) -> Value {
    match (standing, given) {
        (Value::Object(standing), Value::Object(given)) => {
            let mut merged = standing.clone();

            for (key, value) in given {
                let laid = match merged.get(key) {
                    Some(old) => over(old, value),
                    None => value.clone(),
                };
                merged.insert(key.clone(), laid);
            }

            Value::Object(merged)
        }
        (_, given) => given.clone(),
    }
}
