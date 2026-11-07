//! chainv4 send batch: a batch's steps as JSON, in a file or one --step at a time. Each is what the
//! send command for that kind of transaction takes, by the same names, so a step reads as the
//! command would; a name it does not know is an error, not a default, as a genesis file's is.

use {
    super::send::{Recipient, TxArgs, transaction},
    super::{CliResult, parse_amount},
    crate::crypto::verifiable::Verifiable,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::hash::HashType,
    crate::structs::tx::{MAX_BATCH_STEPS, Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    serde_json::Value,
    std::path::{Path, PathBuf},
    ureq::Agent,
};

/// A step's fields, each asked for by name, so what is left over is what was misspelled.
struct Fields<'a> {
    kind: &'a str,
    map: &'a serde_json::Map<String, Value>,
    read: Vec<&'static str>,
}

impl<'a> Fields<'a> {
    fn get(&mut self, name: &'static str) -> Option<&'a Value> {
        self.read.push(name);
        self.map.get(name).filter(|value| !value.is_null())
    }

    fn text(&mut self, name: &'static str) -> CliResult<Option<String>> {
        self.get(name)
            .map(|value| value.as_str().map(str::to_string).ok_or_else(|| format!("a {} step's {name} must be a string", self.kind).into()))
            .transpose()
    }

    fn needed(&mut self, name: &'static str) -> CliResult<String> {
        self.text(name)?.ok_or_else(|| format!("a {} step needs {name}", self.kind).into())
    }

    fn address(&mut self, name: &'static str) -> CliResult<Address> {
        Ok(self.needed(name)?.parse()?)
    }

    fn path(&mut self, name: &'static str) -> CliResult<Option<PathBuf>> {
        Ok(self.text(name)?.map(PathBuf::from))
    }

    /// A whole amount, as a string, or a JSON number short enough to be exact; none when not given.
    fn amount(&mut self, name: &'static str) -> CliResult<Option<u256>> {
        let kind = self.kind;

        self.get(name)
            .map(|value| match value {
                Value::String(text) => Ok(parse_amount(text)?),
                Value::Number(number) if number.is_u64() => Ok(parse_amount(&number.to_string())?),
                _ => Err(format!("a {kind} step's {name} must be a whole amount, as a string").into()),
            })
            .transpose()
    }

    fn whole(&mut self, name: &'static str) -> CliResult<u64> {
        let kind = self.kind;

        self.get(name).and_then(Value::as_u64).ok_or_else(|| format!("a {kind} step needs {name}, a whole number").into())
    }

    fn strings(&mut self, name: &'static str) -> CliResult<Vec<String>> {
        let kind = self.kind;

        match self.get(name) {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => items.iter().map(|item| item.as_str().map(str::to_string).ok_or_else(|| format!("a {kind} step's {name} are strings").into())).collect(),
            Some(_) => Err(format!("a {kind} step's {name} must be a list").into()),
        }
    }

    /// Ends the step: anything it holds that nothing asked for is a mistake.
    fn done(self) -> CliResult<()> {
        match self.map.keys().find(|name| *name != "type" && !self.read.contains(&name.as_str())) {
            Some(name) => Err(format!("a {} step has no {name}", self.kind).into()),
            None => Ok(()),
        }
    }
}

/// One step, as the transaction its send command would make.
fn step(value: &Value, sender: Address, agent: &Agent, node: &str) -> CliResult<Tx> {
    let map = value.as_object().ok_or("a step is a JSON object, such as {\"type\":\"transfer\",\"to\":\"ed25519:0x…\",\"amount\":\"5\"}")?;
    let kind = map.get("type").and_then(Value::as_str).ok_or("a step needs a type")?;
    let mut fields = Fields { kind, map, read: Vec::new() };
    let args = match kind {
        "transfer" => {
            let to: Recipient = fields.needed("to")?.parse()?;

            TxArgs::Transfer { to, amount: fields.amount("amount")?.ok_or("a transfer step needs amount")? }
        }
        "burn" => TxArgs::Burn { amount: fields.amount("amount")?.ok_or("a burn step needs amount")? },
        "call" | "contract_call" => TxArgs::Call {
            to: fields.address("to")?,
            function: fields.needed("function")?,
            amount: fields.amount("amount")?.unwrap_or(u256::ZERO),
            args: fields.strings("args")?.iter().map(|text| TxCallArgumentType::parse_typed(text)).collect::<Result<_, _>>()?,
        },
        "evm_call" => TxArgs::EvmCall {
            to: super::send::evm_to(&fields.needed("to")?)?,
            function: fields.text("function")?,
            data: fields.text("data")?,
            value: fields.amount("value")?.unwrap_or(u256::ZERO),
            read: false,
            args: fields.strings("args")?,
        },
        "deploy" => TxArgs::Deploy {
            lua: fields.path("lua")?,
            wasm: fields.path("wasm")?,
            evm: fields.path("evm")?,
            constructor: fields.text("constructor")?,
            constructor_args: fields.strings("args")?,
        },
        // Asked about as chainv4 send remove asks, when the step is read.
        "remove" => TxArgs::Remove {
            contract: fields.address("contract")?,
            to: fields.address("to")?,
            yes: false,
        },
        "upgrade" => TxArgs::Upgrade {
            contract: fields.address("contract")?,
            lua: fields.path("lua")?,
            wasm: fields.path("wasm")?,
        },
        #[cfg(feature = "names")]
        "name" | "primary_name" => {
            use super::send::NameArgs;

            let action = match kind {
                "primary_name" => NameArgs::Primary { name: fields.text("name")? },
                _ => {
                    let name = fields.needed("name")?;
                    let years = fields.get("years").and_then(Value::as_u64).map_or(Ok(1), u32::try_from)?;

                    match fields.needed("action")?.as_str() {
                        "register" => NameArgs::Register { name, years },
                        "renew" => NameArgs::Renew { name, years },
                        "transfer" => NameArgs::Transfer { name, to: fields.address("to")? },
                        "target" => {
                            let to = fields.text("to")?.map(|to| to.parse()).transpose()?;

                            NameArgs::Target { name, none: to.is_none(), to }
                        }
                        "text" => NameArgs::Text { name, key: fields.needed("key")?, value: fields.text("value")? },
                        "subname" => NameArgs::Subname { name, label: fields.needed("label")?, owner: fields.text("owner")?.map(|owner| owner.parse()).transpose()? },
                        other => return Err(format!("a name step's action is register, renew, transfer, target, text or subname, not {other}").into()),
                    }
                }
            };

            TxArgs::Name { action }
        }
        "claim" => {
            let (height, to, earned, index) = (fields.whole("height")?, fields.address("to")?, fields.amount("earned")?.ok_or("a claim step needs earned")?, fields.whole("index")?);
            let path = fields
                .strings("path")?
                .iter()
                .map(|text| crate::structs::hex::from_hex(text).and_then(|bytes| Hash::from_bytes(HashType::default_chain(), &bytes)).ok_or_else(|| format!("{text} is not one of a claim's hashes").into()))
                .collect::<CliResult<Vec<Hash>>>()?;
            fields.done()?;

            return Ok(Tx::Claim { height, to, earned, index, path });
        }
        other => return Err(format!("a batch holds transfer, burn, call, evm_call, deploy, upgrade, remove, name, primary_name and claim steps, not {other}").into()),
    };
    fields.done()?;

    transaction(args, sender, agent, node)
}

/// What --file and --step hold, in that order.
fn read(steps: &[String], file: Option<&Path>) -> CliResult<Vec<Value>> {
    let mut values = Vec::new();

    if let Some(path) = file {
        let text = std::fs::read_to_string(path).map_err(|err| format!("cannot read {}: {err}", path.display()))?;

        match serde_json::from_str(&text).map_err(|err| format!("{} is not JSON: {err}", path.display()))? {
            Value::Array(listed) => values.extend(listed),
            _ => return Err(format!("{} must hold a list of steps", path.display()).into()),
        }
    }

    for text in steps {
        values.push(serde_json::from_str(text).map_err(|err| format!("a --step is not JSON: {err}: {text}"))?);
    }

    match values.len() {
        0 => Err("a batch needs a step: give --file or --step".into()),
        count if count > MAX_BATCH_STEPS => Err(format!("a batch holds at most {MAX_BATCH_STEPS} steps; this has {count}").into()),
        _ => Ok(values),
    }
}

/// The batch these steps make, each from this sender, checked here so a mistake is named before
/// anything is sent.
pub(super) fn batch(steps: &[String], file: Option<&Path>, sender: Address, agent: &Agent, node: &str) -> CliResult<Tx> {
    let mut made = Vec::new();

    for (index, value) in read(steps, file)?.iter().enumerate() {
        made.push(step(value, sender, agent, node).map_err(|err| format!("step {}: {err}", index + 1))?);
    }

    let tx = Tx::Batch { steps: made };
    tx.verify()?;

    Ok(tx)
}
