//! Genesis files: a chain's whole configuration as JSON, so every parameter can be chosen without
//! touching code. `chainv4 genesis` writes one with the development defaults; `chainv4 --genesis
//! FILE` runs a node on it. Every field must be present, and unknown fields are refused, so a
//! misspelled parameter is an error rather than a silent default.

use {
    super::CliResult,
    crate::chain::{Allocation, FinalityConfig, Genesis, GenesisValidator, Reward},
    crate::consensus::{ConsensusConfig, PowConfig},
    crate::structs::hash::HashType,
    crate::crypto::keypair::PublicKey,
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
    /// Leave finality out.
    #[arg(long)]
    pub no_finality: bool,
    /// Where to write it. Without one, it goes to stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum DevChain {
    Pow,
    Authority,
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

    if args.no_finality {
        genesis.finality = None;
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
        "max_reorg_depth": genesis.max_reorg_depth,
        "finality": genesis.finality.as_ref().map(|finality| json!({
            "checkpoint_interval": finality.checkpoint_interval,
            "vote_depth": finality.vote_depth,
            "min_bond": finality.min_bond.to_string(),
            "activation_delay": finality.activation_delay,
            "withdrawal_delay": finality.withdrawal_delay,
            "reporter_divisor": finality.reporter_divisor,
            "max_validators": finality.max_validators,
            "vote_reward": finality.vote_reward.to_string(),
            "max_proofs_per_block": finality.max_proofs_per_block,
            "validators": finality.validators.iter().map(|validator| json!({
                "account": validator.account.to_string(),
                "voter": validator.voter.to_string(),
                "bond": validator.bond.to_string(),
            })).collect::<Vec<_>>(),
        })),
    })
}

pub fn from_json(value: &Value) -> Result<Genesis, String> {
    let mut root = Fields::of(value, "genesis")?;
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
    let max_reorg_depth = root.whole("max_reorg_depth")?;
    let finality = match root.optional_object("finality")? {
        None => None,
        Some(mut finality) => {
            let config = FinalityConfig {
                checkpoint_interval: finality.whole("checkpoint_interval")?,
                vote_depth: finality.whole("vote_depth")?,
                min_bond: finality.amount("min_bond")?,
                activation_delay: finality.whole("activation_delay")?,
                withdrawal_delay: finality.whole("withdrawal_delay")?,
                reporter_divisor: finality.whole("reporter_divisor")?,
                max_validators: u32::try_from(finality.whole("max_validators")?)
                    .map_err(|_| "finality.max_validators is too large".to_string())?,
                vote_reward: finality.amount("vote_reward")?,
                max_proofs_per_block: u32::try_from(finality.whole("max_proofs_per_block")?)
                    .map_err(|_| "finality.max_proofs_per_block is too large".to_string())?,
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
            };
            finality.done()?;

            Some(config)
        }
    };
    let genesis = Genesis {
        timestamp: root.whole("timestamp")?,
        hash: root.hash("hash")?,
        consensus,
        reward: reward_value,
        minter,
        allocations,
        engines,
        tx_limits,
        max_reorg_depth,
        finality,
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

    #[test]
    fn genesis_files_round_trip_exactly() {
        for chain in [DevChain::Pow, DevChain::Authority] {
            let genesis = dev(chain);
            let mut bare = genesis.clone();
            bare.finality = None;

            for genesis in [genesis, bare] {
                let read = from_json(&to_json(&genesis)).unwrap();

                assert_eq!(read, genesis);
                assert_eq!(read.block().id(), genesis.block().id());
            }
        }
    }

    #[test]
    fn every_parameter_can_change_and_changes_the_chain() {
        let genesis = dev(DevChain::Pow);
        let mut value = to_json(&genesis);
        value["finality"]["checkpoint_interval"] = json!(32);
        value["finality"]["activation_delay"] = json!(4);
        value["tx_limits"]["fuel"] = json!(1_000);
        value["max_reorg_depth"] = json!(500);
        let changed = from_json(&value).unwrap();
        let finality = changed.finality.as_ref().unwrap();

        assert_eq!((finality.checkpoint_interval, finality.activation_delay), (32, 4));
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
    fn mistakes_are_named() {
        let cases: [(&str, Value, &str); 5] = [
            ("/finality/checkpont_interval", json!(8), "finality.checkpont_interval is not a genesis field"),
            ("/max_reorg_depth", json!("100"), "max_reorg_depth must be a whole number"),
            ("/engines", json!(["lua", "evm"]), "unknown engine evm"),
            ("/consensus/hash", json!("sha1"), "consensus.hash: unknown hash sha1"),
            ("/allocations/0/amount", json!("-5"), "allocations[0].amount: -5 is not a whole amount"),
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

        let mut missing = to_json(&dev(DevChain::Pow));
        missing["finality"].as_object_mut().unwrap().remove("vote_depth");

        assert_eq!(from_json(&missing).unwrap_err(), "finality.vote_depth is missing");
    }
}
