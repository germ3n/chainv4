//! An EVM chain's JSON-RPC, as Ethereum's nodes and providers serve it, for a swap's EVM leg: what
//! Swap.sol holds, and transactions signed here, sent and waited for. Any chain's will do, the
//! node's own POST /rpc too.

use {
    super::super::CliResult,
    crate::crypto::secp256k1,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::u256::u256,
    crate::swap::evm::Call,
    serde_json::{Value, json},
    std::time::{Duration, Instant},
};

/// One chain's JSON-RPC, by its URL.
pub struct Rpc {
    agent: ureq::Agent,
    url: String,
}

impl Rpc {
    pub fn new(agent: ureq::Agent, url: &str) -> Rpc {
        Rpc { agent, url: url.trim().to_string() }
    }

    /// The first of the URLs whose chain is this one.
    pub fn for_chain(agent: &ureq::Agent, urls: &[String], chain_id: u64) -> CliResult<Rpc> {
        for url in urls {
            let rpc = Rpc::new(agent.clone(), url);

            if rpc.chain_id().is_ok_and(|id| id == chain_id) {
                return Ok(rpc);
            }
        }

        Err(format!("no --eth-rpc given answers for {}: give one of its JSON-RPC URLs", crate::swap::evm::chain_name(chain_id)).into())
    }

    fn ask(&self, method: &str, params: Value) -> CliResult<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut response = self
            .agent
            .post(&self.url)
            .header("content-type", "application/json")
            .send(body.to_string())
            .map_err(|err| format!("cannot reach {}: {err}", self.url))?;
        let answer: Value = serde_json::from_str(&response.body_mut().read_to_string()?).map_err(|_| format!("{} did not answer JSON-RPC", self.url))?;

        match answer.get("error") {
            Some(error) if !error.is_null() => Err(format!("{method}: {}", error["message"].as_str().unwrap_or(&error.to_string())).into()),
            _ => Ok(answer["result"].clone()),
        }
    }

    fn number(&self, method: &str, params: Value) -> CliResult<u256> {
        let answer = self.ask(method, params)?;
        let hex = answer.as_str().and_then(|text| text.strip_prefix("0x")).ok_or_else(|| format!("{method} answered {answer}"))?;

        Ok(u256::from_str_radix(if hex.is_empty() { "0" } else { hex }, 16).map_err(|_| format!("{method} answered {answer}"))?)
    }

    pub fn chain_id(&self) -> CliResult<u64> {
        u64::try_from(self.number("eth_chainId", json!([]))?).map_err(|_| "a chain id past 64 bits".into())
    }

    /// What a call to a contract returns, at the latest block.
    pub fn call(&self, to: &[u8; 20], input: &[u8]) -> CliResult<Vec<u8>> {
        let answer = self.ask("eth_call", json!([{ "to": to_hex(to), "data": to_hex(input) }, "latest"]))?;

        Ok(answer.as_str().and_then(from_hex).ok_or_else(|| format!("eth_call answered {answer}"))?)
    }

    /// The chain's height, and the block its RPC calls finalized, where it names one.
    pub fn heads(&self) -> CliResult<(u64, Option<u64>)> {
        let tip = u64::try_from(self.number("eth_blockNumber", json!([]))?).map_err(|_| "a height past 64 bits")?;
        let finalized = self
            .ask("eth_getBlockByNumber", json!(["finalized", false]))
            .ok()
            .and_then(|block| block["number"].as_str().and_then(|hex| u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok()));

        Ok((tip, finalized))
    }

    /// Whether there is code at an address.
    pub fn has_code(&self, address: &[u8; 20]) -> CliResult<bool> {
        let answer = self.ask("eth_getCode", json!([to_hex(address), "latest"]))?;

        Ok(answer.as_str().is_some_and(|code| code.len() > 2))
    }

    /// Signs a call with a secp256k1 key, sends it and waits for it to be mined: its receipt, or
    /// why it failed. A call to no one creates its input's contract.
    pub fn send(&self, call: &Call, create: bool, secret: &[u8; 32]) -> CliResult<Value> {
        let chain_id = self.chain_id()?;
        let from = from_key(secret)?;
        let nonce = u64::try_from(self.number("eth_getTransactionCount", json!([to_hex(&from), "pending"]))?).map_err(|_| "a nonce past 64 bits")?;
        let price = self.number("eth_gasPrice", json!([]))?;
        // Not every chain's RPC suggests a tip; one at the whole price then pays no more.
        let tip = self.number("eth_maxPriorityFeePerGas", json!([])).unwrap_or(price).min(price);
        let mut estimate = json!({ "from": to_hex(&from), "value": format!("{:#x}", call.value), "data": to_hex(&call.input) });
        if !create {
            estimate["to"] = json!(to_hex(&call.to));
        }
        let gas = u64::try_from(self.number("eth_estimateGas", json!([estimate]))?).map_err(|_| "a gas estimate past 64 bits")?;
        // Room above the estimate, and above a base fee that rises for a few blocks.
        let mut tx = call.transaction(chain_id, nonce, gas + gas / 4 + 10_000, price * u256::new(2), tip);
        if create {
            tx.to = None;
        }
        let hash = self.ask("eth_sendRawTransaction", json!([to_hex(&tx.signed(secret).encode())]))?;
        let hash = hash.as_str().ok_or_else(|| format!("eth_sendRawTransaction answered {hash}"))?.to_string();
        println!("{}: sent {hash}", call.what);
        let (started, mut pause) = (Instant::now(), Duration::from_millis(250));

        loop {
            let receipt = self.ask("eth_getTransactionReceipt", json!([hash]))?;

            if !receipt.is_null() {
                return match receipt["status"].as_str() {
                    Some("0x1") => Ok(receipt),
                    _ => Err(format!("{} was mined but failed: {hash}", call.what).into()),
                };
            }

            if started.elapsed() > Duration::from_secs(600) {
                return Err(format!("{hash} is not mined after ten minutes: look it up, and try again once it is").into());
            }

            // Soon on a fast chain, and no more than every few seconds on a slow one.
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_secs(4));
        }
    }

    /// Deploys init code: through the deterministic deployment proxy when the chain has it, so it
    /// lands where CREATE2 puts it, or else as a plain create. Where it landed.
    pub fn deploy(&self, init: &[u8], secret: &[u8; 32]) -> CliResult<[u8; 20]> {
        use crate::swap::evm::{DEPLOYMENT_PROXY, create2, parse_address};

        if !self.has_code(&DEPLOYMENT_PROXY)? {
            let receipt = self.send(&Call { to: [0; 20], value: u256::ZERO, input: init.to_vec(), what: "Deploy" }, true, secret)?;

            return Ok(parse_address(receipt["contractAddress"].as_str().ok_or("the deploy's receipt names no contract")?)?);
        }

        let address = create2(&DEPLOYMENT_PROXY, &[0; 32], init);

        if !self.has_code(&address)? {
            self.send(&Call { to: DEPLOYMENT_PROXY, value: u256::ZERO, input: [&[0; 32][..], init].concat(), what: "Deploy through the proxy" }, false, secret)?;
        }

        Ok(address)
    }
}

/// The EVM address of a secp256k1 key.
pub fn from_key(secret: &[u8; 32]) -> CliResult<[u8; 20]> {
    let public = secp256k1::public_key(secret).ok_or("a key that is not a secp256k1 key")?;

    Ok(secp256k1::address(&public).ok_or("a key with no address")?)
}
