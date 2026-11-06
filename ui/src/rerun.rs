//! A contract read run here, on the desktop and Android, over state the node proves, rather than
//! taken on the node's word, for the wallet and the explorer. Every read an app sends asks the node to run it at the block its
//! light client checked last, and to send every state entry it read with its proof
//! (`POST /call` with `"prove"`). The answer it gets is then this app's own: the call run by its
//! own engines over those entries alone, each checked against that block's state root, put in place
//! of the node's, so every page that reads a contract shows what the chain holds. The browser has no
//! Lua engine, so there the node's answer stands, and says so.

use {
    chainv4::db::{STATE_COLUMNS, StateProof},
    chainv4::light::LightClient,
    chainv4::params::vm::{DexSettings, VmRules},
    chainv4::structs::address::Address,
    chainv4::structs::code::Engine,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::{from_hex, to_hex},
    chainv4::structs::u256::u256,
    chainv4::vm::evm::{EvmBlock, EvmReturned},
    chainv4::vm::{BlockAt, CallContext, EvmRead, Limits, ProvenRead, Vm, run_proven, run_proven_evm},
    serde_json::{Value, json},
    std::sync::Arc,
};

/// What a read run here may use: more than any node gives one, so a read the node finished never
/// runs out here first.
const LIMITS: Limits = Limits { fuel: 10_000_000_000, memory: 256 * 1024 * 1024 };

/// What the wallet's own reads go by beside the proven state: what /status says the chain runs
/// with, the node's word until genesis is proven too.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    pub dex_fee_bps: u32,
    pub evm_spec: Option<chainv4::params::vm::EvmSpec>,
}

/// The engines reads run on here: the app keeps one, made the first time a read is checked.
#[derive(Default)]
pub struct Engines(Option<Arc<Vm>>);

impl Engines {
    fn get(&mut self, hash: HashType) -> Option<Arc<Vm>> {
        if self.0.is_none() {
            self.0 = Vm::new(&[Engine::Lua, Engine::Wasm, Engine::Evm], hash).ok().map(Arc::new);
        }

        self.0.clone()
    }
}

/// The request's body as sent: the read, at the block the light client checked last, by its id,
/// with the entries it reads proven.
pub fn proving(body: &str, block: &str) -> Option<String> {
    let mut body: Value = serde_json::from_str(body).ok()?;
    body["prove"] = true.into();
    body["block"] = block.into();

    Some(body.to_string())
}

/// The node's answer to a read, made this app's own: run here over what it proved, the outcome
/// in place of the node's, and "checked" saying so; or, where it cannot be run so, the node's
/// answer with "checked" saying why.
pub fn checked(engines: &mut Engines, body: &str, answer: Value, client: Option<&LightClient>, rules: Rules) -> Value {
    match run_here(engines, body, &answer, client, rules) {
        Ok(mine) => {
            let differs = ["status", "values", "output", "error"].iter().any(|field| mine[*field] != answer[*field]);
            let mut ours = answer;

            for field in ["status", "values", "output", "error", "reverted"] {
                ours[field] = mine[field].clone();
            }
            ours["checked"] = json!({ "here": true, "height": mine["height"], "differed": differs });

            ours
        }
        Err(why) => {
            let mut theirs = answer;
            theirs["checked"] = json!({ "here": false, "why": why });

            theirs
        }
    }
}

fn run_here(engines: &mut Engines, body: &str, answer: &Value, client: Option<&LightClient>, rules: Rules) -> Result<Value, String> {
    let client = client.ok_or("the light client has not started")?;
    let proven = answer.get("proven").ok_or("the node sent no proofs: an older node")?;
    let body: Value = serde_json::from_str(body).map_err(|_| "a read that does not read")?;
    let hash = client.params().hash;
    let height = proven["height"].as_u64().ok_or("proofs at no height")?;
    let header = client.header_at(height).ok_or("proven at a block the light client does not hold")?;

    if proven["block"].as_str() != Some(&header.id().to_string()) {
        return Err("proven at a block that is not the one the light client checked".to_string());
    }

    if proven["scanned"].as_bool() == Some(true) {
        return Err("the read lists a range of storage, which cannot be proven entry by entry yet".to_string());
    }

    if proven["unprovable"].as_array().is_some_and(|columns| !columns.is_empty()) {
        return Err("the read goes by something besides the chain's state".to_string());
    }

    let reads = proven["entries"].as_array().into_iter().flatten().map(read).collect::<Result<Vec<_>, _>>()?;
    let mut vm_rules = VmRules { dex: DexSettings { fee_bps: rules.dex_fee_bps }, ..VmRules::default() };

    if let Some(spec) = rules.evm_spec {
        vm_rules.evm_spec = spec;
    }

    // Calldata: an EVM read, at the block's own height and time, as the node ran it.
    if let Some(input) = body.get("input") {
        let hex = |value: &Value| value.as_str().and_then(from_hex);
        let to = match body["contract"].as_str().and_then(|text| text.parse::<Address>().ok()) {
            Some(Address::Evm(to)) => to,
            _ => return Err("calldata to no EVM contract".to_string()),
        };
        let from: [u8; 20] = match body.get("from") {
            None | Some(Value::Null) => [0; 20],
            Some(from) => hex(from).and_then(|bytes| bytes.try_into().ok()).ok_or("a sender that is no EVM address")?,
        };
        let read = EvmRead { from, to, input: hex(input).ok_or("calldata that is not hex")? };
        let block = EvmBlock {
            height,
            timestamp: header.timestamp,
            producer: header.producer,
            chain_id: chainv4::crypto::ethereum::chain_id(&client.genesis()),
            // The names precompile goes by the chain's names, which nothing proven here says: a
            // read that resolves a name shows as differing from the node.
            names: None,
            hash,
            vm: vm_rules,
        };
        let vm = engines.get(hash).ok_or("this build runs no contracts")?;
        let ran = run_proven_evm(&vm, &header.state_root, &reads, &read, block, LIMITS.memory as u64).map_err(|why| why.to_string())?;
        let mut mine = match &ran.result {
            Ok(EvmReturned::Output(output)) => json!({ "status": "returned", "output": to_hex(output), "error": Value::Null }),
            Ok(EvmReturned::Created(_)) => json!({ "status": "returned", "output": "0x", "error": Value::Null }),
            Err(err) => json!({ "status": "failed", "output": Value::Null, "error": err.to_string(), "reverted": to_hex(&ran.reverted) }),
        };
        mine["height"] = height.into();

        return Ok(mine);
    }

    let contract: Address = body["contract"].as_str().and_then(|text| text.parse().ok()).ok_or("a read of no contract")?;
    let function = Hash::Xxh3_64(chainv4::vm::selector(body["function"].as_str().ok_or("a read of no function")?));
    let args = body["args"].as_array().into_iter().flatten().map(chainv4::structs::json::parse_argument).collect::<Result<Vec<_>, _>>()?;
    let sender = body["sender"].as_str().and_then(|text| text.parse().ok()).unwrap_or(Address::Ed25519([0; 32]));
    let vm = engines.get(hash).ok_or("this build runs no contracts")?;
    let context = CallContext {
        sender,
        contract,
        value: u256::ZERO,
        block: BlockAt { height, time: header.timestamp, producer: header.producer, fuel_per_gas: 1, vm: vm_rules },
    };
    let outcome = run_proven(&vm, &header.state_root, &reads, &contract, &function, &args, context, LIMITS).map_err(|why| why.to_string())?;
    let mut mine = match &outcome.result {
        Ok(values) => json!({ "status": "returned", "values": values.iter().map(chainv4::structs::json::argument).collect::<Vec<_>>(), "error": Value::Null }),
        Err(err) => json!({ "status": "failed", "values": Value::Null, "error": err.to_string() }),
    };
    mine["height"] = height.into();

    Ok(mine)
}

/// One entry the node proved, as it wrote it.
fn read(entry: &Value) -> Result<ProvenRead, String> {
    let named = entry["column"].as_str().unwrap_or_default();
    let column = STATE_COLUMNS.into_iter().find(|column| column.name() == named).ok_or_else(|| format!("{named} is no state column"))?;
    let key = entry["key"].as_str().and_then(from_hex).ok_or("an entry with no key")?;
    let value = match &entry["value"] {
        Value::Null => None,
        Value::String(text) => Some(from_hex(text).ok_or("a value that is not hex")?),
        _ => return Err("a value that is not hex".to_string()),
    };
    let proof: StateProof = entry["proof"].as_str().and_then(from_hex).and_then(|bytes| wincode::deserialize(&bytes).ok()).ok_or("a proof that does not decode")?;

    Ok(ProvenRead { column, key, value, proof })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_goes_out_to_be_proven_and_one_that_cannot_be_run_here_keeps_the_nodes_answer_saying_why() {
        let body = json!({ "contract": "contract:0x01", "function": "balance_of", "args": [] }).to_string();
        let sent: Value = serde_json::from_str(&proving(&body, "0xabc").unwrap()).unwrap();
        assert_eq!((sent["prove"].as_bool(), sent["block"].as_str(), sent["function"].as_str()), (Some(true), Some("0xabc"), Some("balance_of")));
        assert!(proving("not json", "0xabc").is_none());

        let rules = Rules { dex_fee_bps: 30, evm_spec: None };
        let answer = json!({ "status": "returned", "values": [{ "type": "i64", "value": "1" }] });
        let mut engines = Engines::default();

        // No light client yet: the node's word, and why.
        let kept = checked(&mut engines, &body, answer.clone(), None, rules);
        assert_eq!(kept["values"], answer["values"]);
        assert_eq!(kept["checked"]["here"], false);
        assert!(kept["checked"]["why"].as_str().unwrap().contains("light client"));
    }

    #[test]
    fn an_entry_a_node_sent_reads_back_as_it_was_proven() {
        let entry = json!({ "column": "storage", "key": "0x0102", "value": null, "proof": "0x00" });
        assert!(read(&entry).is_err(), "a proof that does not decode");
        let entry = json!({ "column": "meta", "key": "0x01", "value": null, "proof": "0x00" });
        assert!(read(&entry).unwrap_err().contains("no state column"));
    }
}
