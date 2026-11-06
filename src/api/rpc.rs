//! Ethereum's JSON-RPC, at POST /rpc: what MetaMask, Foundry and ethers ask a node, answered from
//! chainv4's own chain. Balances and nonces are every account's, at its EVM address; blocks are
//! chainv4's, listing the Ethereum-signed transactions in them; a transaction is found by the
//! keccak256 of its signed bytes, which the node indexes as it lands. Only the latest state is
//! kept: a question about an older block's state is refused.

use {
    super::{Api, ApiError, tip},
    crate::chain::{stored_block, stored_block_at, stored_ethereum_tx, stored_header_at, stored_receipt, stored_state_at},
    crate::chain::{TxReceipt, TxStatus},
    crate::crypto::ethereum::{EthereumKind, EthereumTx, chain_id, decode, tx_hash},
    crate::db::State,
    crate::mempool::MempoolError,
    crate::node::SubmitError,
    crate::registry::Accounts,
    crate::registry::contracts::ContractRecord,
    crate::structs::address::Address,
    crate::structs::block::{Block, BlockHeader},
    crate::structs::hash::Hash,
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    crate::vm::evm::{self, EvmAction, EvmBlock, EvmRan, LOG, account_of},
    serde_json::{Value, json},
    std::collections::HashSet,
    std::sync::Arc,
};

/// The most places a log has topics at: a filter asking past them never matches.
const LOG_TOPIC_PLACES: usize = 4;
// The rest of what /rpc answers within, the blocks one eth_getLogs scans, the addresses and topics
// a filter may name, a batch's requests and eth_feeHistory's blocks, are RpcLimits, the node's own.

/// A JSON-RPC error: its code, as Ethereum's nodes use them, its message, and any data.
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn invalid(message: impl Into<String>) -> RpcError {
        RpcError { code: -32602, message: message.into(), data: None }
    }

    fn server(message: impl Into<String>) -> RpcError {
        RpcError { code: -32000, message: message.into(), data: None }
    }
}

impl From<ApiError> for RpcError {
    fn from(err: ApiError) -> RpcError {
        let message = match err {
            ApiError::BadRequest(message) | ApiError::NotFound(message) | ApiError::Conflict(message) | ApiError::Unavailable(message) | ApiError::TooMany(message) | ApiError::Forbidden(message) => message,
            ApiError::Db(err) => err.to_string(),
        };

        RpcError::server(message)
    }
}

impl From<crate::db::DbError> for RpcError {
    fn from(err: crate::db::DbError) -> RpcError {
        RpcError::server(err.to_string())
    }
}

type Answer = Result<Value, RpcError>;

/// POST /rpc: one request, or a batch of them, each answered in order.
pub(super) async fn rpc(api: Arc<Api>, body: &[u8]) -> Value {
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return reply(&Value::Null, Err(RpcError { code: -32700, message: "the body is not JSON".to_string(), data: None }));
    };

    match request {
        Value::Array(batch) if batch.is_empty() => reply(&Value::Null, Err(RpcError { code: -32600, message: "an empty batch".to_string(), data: None })),
        Value::Array(batch) if batch.len() > api.tuned.rpc.batch => reply(&Value::Null, Err(RpcError { code: -32600, message: format!("a batch holds at most {} requests", api.tuned.rpc.batch), data: None })),
        Value::Array(batch) => {
            let mut replies = Vec::with_capacity(batch.len());

            for request in &batch {
                replies.push(one(&api, request).await);
            }

            Value::Array(replies)
        }
        request => one(&api, &request).await,
    }
}

async fn one(api: &Arc<Api>, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return reply(&id, Err(RpcError { code: -32600, message: "a request names its method".to_string(), data: None }));
    };
    let params = request.get("params").and_then(Value::as_array).cloned().unwrap_or_default();

    reply(&id, answer(api, method, &params).await)
}

fn reply(id: &Value, answer: Answer) -> Value {
    match answer {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(err) => {
            let mut error = json!({ "code": err.code, "message": err.message });

            if let Some(data) = err.data {
                error["data"] = data;
            }

            json!({ "jsonrpc": "2.0", "id": id, "error": error })
        }
    }
}

async fn answer(api: &Arc<Api>, method: &str, params: &[Value]) -> Answer {
    let db = api.db.clone();
    let param = |at: usize| params.get(at).unwrap_or(&Value::Null);

    match method {
        "web3_clientVersion" => Ok(format!("chainv4/{}", env!("CARGO_PKG_VERSION")).into()),
        "net_version" => Ok(chain(api)?.to_string().into()),
        "net_listening" => Ok(true.into()),
        "eth_chainId" => Ok(quantity(u256::from(chain(api)?))),
        "eth_blockNumber" => Ok(quantity(u256::from(tip(&*db)?.header.height))),
        "eth_syncing" => Ok(false.into()),
        "eth_accounts" | "eth_requestAccounts" => Ok(json!([])),
        "eth_gasPrice" => Ok(quantity(next_base(api)?.saturating_mul(u256::from(api.tuned.suggest)))),
        "eth_maxPriorityFeePerGas" => Ok(quantity(u256::ZERO)),
        "eth_feeHistory" => fee_history(api, param(0), param(1), param(2)),
        "eth_getBalance" => {
            let (state, _) = state_at(api, param(1))?;
            let account = account_of(&state, address(param(0))?)?;

            Ok(quantity(Accounts::new(&state).get(&account)?.balance))
        }
        "eth_getTransactionCount" => {
            let pending = param(1).as_str() == Some("pending");
            let (state, _) = state_at(api, param(1))?;
            let bytes = address(param(0))?;
            let account = account_of(&state, bytes)?;
            let mut nonce = Accounts::new(&state).get(&account)?.nonce;

            // Those it sent that wait in the mempool take the nonces after: the pending count is one
            // past the highest it has waiting, when that is higher than the state nonce.
            if pending && let Some(node) = &api.node {
                let answer = node.mempool(crate::node::MempoolQuery::HighestNonce(account)).await.map_err(|err| RpcError::server(err.to_string()))?;

                if let crate::node::MempoolAnswer::HighestNonce(Some(highest)) = answer {
                    nonce = nonce.max(highest.saturating_add(1));
                }
            }

            Ok(quantity(u256::from(nonce)))
        }
        "eth_getCode" => {
            let (state, _) = state_at(api, param(1))?;
            let record: Option<ContractRecord> = state.get_value(crate::db::Column::Contracts, &crate::db::address_key(&Address::Evm(address(param(0))?)))?;

            Ok(to_hex(&record.filter(|record| record.engine == crate::structs::code::Engine::Evm).map(|record| record.code).unwrap_or_default()).into())
        }
        "eth_getStorageAt" => {
            let (state, _) = state_at(api, param(2))?;
            let slot = word(param(1))?;

            Ok(to_hex(&evm::storage_word(&state, address(param(0))?, slot)?).into())
        }
        "eth_call" => {
            let at = state_at(api, param(1))?;
            let call = Call::read(param(0))?;
            let ran = simulate(api, call, at).await?;

            match ran.result {
                Ok(evm::EvmReturned::Output(output)) => Ok(to_hex(&output).into()),
                Ok(evm::EvmReturned::Created(_)) => Ok("0x".into()),
                Err(err) => Err(failed(&err.to_string(), &ran.reverted)),
            }
        }
        "eth_estimateGas" => {
            let call = Call::read(param(0))?;
            let ran = simulate(api, call.clone(), state_at(api, &Value::Null)?).await?;

            if let Err(err) = &ran.result {
                return Err(failed(&err.to_string(), &ran.reverted));
            }

            // What it used may be too little to succeed with, as a call inside another gets
            // 63/64 of what is left: tried at that, and raised until a run succeeds.
            let mut needed = ran.gas_used;

            for _ in 0..16 {
                let tried = simulate(api, Call { gas: Some(needed), ..call.clone() }, state_at(api, &Value::Null)?).await?;

                if tried.result.is_ok() {
                    break;
                }

                needed = needed.saturating_add(needed / 4).max(needed + 1);
            }

            Ok(quantity(u256::from(estimate(api, &call, needed))))
        }
        "eth_sendRawTransaction" => {
            let raw = bytes(param(0), "the transaction")?;
            let hash = tx_hash(&raw);
            let signed = SignedTx::from_ethereum(raw).map_err(RpcError::invalid)?;
            let ethereum = decode(match &signed.tx {
                Tx::Ethereum { raw } => raw,
                _ => unreachable!("from_ethereum makes Tx::Ethereum"),
            })
            .map_err(RpcError::invalid)?;

            if ethereum.chain_id != chain(api)? {
                return Err(RpcError::invalid(format!("signed for chain {}, not this one, {}", ethereum.chain_id, chain(api)?)));
            }

            let node = api.node.as_ref().ok_or_else(|| RpcError::server("this API has no node to send to"))?;

            match node.submit(signed).await {
                Ok(_) => Ok(to_hex(&hash).into()),
                Err(SubmitError::Refused(err)) => Err(RpcError::server(match err {
                    MempoolError::Stale => "nonce too low".to_string(),
                    MempoolError::Duplicate => "already known".to_string(),
                    MempoolError::PriceTooLow => "max fee per gas less than block base fee".to_string(),
                    MempoolError::CannotPay => "insufficient funds for gas * price + value".to_string(),
                    MempoolError::BadGasLimit => "intrinsic gas too low, or past the block gas limit".to_string(),
                    // As Ethereum's nodes word them, which wallets know to act on.
                    MempoolError::ReplacementTooLow { bump } => format!("replacement transaction underpriced: raise both the max fee per gas and the priority fee by at least {bump}%"),
                    MempoolError::TooFarAhead { next, most } => format!("nonce too high: the account's next is {next}, and this node takes nonces at most {most} past it"),
                    MempoolError::TooMany { most } => format!("account limit exceeded: {most} transactions from this account wait already"),
                    MempoolError::Full => "txpool is full".to_string(),
                    other => other.to_string(),
                })),
                Err(err) => Err(RpcError::server(err.to_string())),
            }
        }
        "eth_getTransactionByHash" => match found(api, param(0))? {
            Some(found) => Ok(found.tx_json(api)),
            None => pending(api, param(0)).await,
        },
        "eth_getTransactionReceipt" => Ok(found(api, param(0))?.map_or(Value::Null, |found| found.receipt_json(api))),
        "eth_getBlockByNumber" => Ok(block_at(api, param(0))?.map_or(Value::Null, |block| block_json(api, &block, param(1).as_bool().unwrap_or(false)))),
        "eth_getBlockByHash" => Ok(block_by_hash(api, param(0))?.map_or(Value::Null, |block| block_json(api, &block, param(1).as_bool().unwrap_or(false)))),
        "eth_getBlockTransactionCountByNumber" => Ok(block_at(api, param(0))?.map_or(Value::Null, |block| quantity(u256::from(ethereum_txs(&block).count() as u64)))),
        "eth_getBlockTransactionCountByHash" => Ok(block_by_hash(api, param(0))?.map_or(Value::Null, |block| quantity(u256::from(ethereum_txs(&block).count() as u64)))),
        "eth_getLogs" => logs(api, param(0)),
        _ => Err(RpcError { code: -32601, message: format!("the method {method} is not served here"), data: None }),
    }
}

/// The chain id transactions are signed for.
fn chain(api: &Api) -> Result<u64, RpcError> {
    let genesis = stored_header_at(&*api.db, 0)?.ok_or_else(|| RpcError::server("the chain has no genesis yet"))?;

    Ok(chain_id(&genesis.id()))
}

/// What the next block's base price is: what a transaction sent now must pay at least.
fn next_base(api: &Api) -> Result<u256, RpcError> {
    let header = tip(&*api.db)?.header;

    Ok(api.fees.as_ref().map_or(u256::ZERO, |fees| fees.pricing().next_price(header.base_price, header.gas_used, fees.block_gas_limit())))
}

/// A whole number as JSON-RPC writes one: 0x and hex, no leading zeros.
fn quantity(value: u256) -> Value {
    format!("{value:#x}").into()
}

/// A whole number param: 0x and hex.
fn number(value: &Value, what: &str) -> Result<u256, RpcError> {
    let text = value.as_str().ok_or_else(|| RpcError::invalid(format!("{what} is a hex quantity")))?;
    let digits = text.strip_prefix("0x").ok_or_else(|| RpcError::invalid(format!("{what} starts with 0x")))?;

    match digits {
        "" => Ok(u256::ZERO),
        digits => u256::from_str_radix(digits, 16).map_err(|_| RpcError::invalid(format!("{what} is not hex"))),
    }
}

fn bytes(value: &Value, what: &str) -> Result<Vec<u8>, RpcError> {
    value.as_str().and_then(from_hex).ok_or_else(|| RpcError::invalid(format!("{what} is hex bytes")))
}

fn address(value: &Value) -> Result<[u8; 20], RpcError> {
    bytes(value, "an address")?.try_into().map_err(|_| RpcError::invalid("an address is 20 bytes"))
}

fn word(value: &Value) -> Result<[u8; 32], RpcError> {
    Ok(number(value, "a slot")?.to_be_bytes())
}

/// The state at the block a question about state names, and its header: latest's and pending's
/// as it is; a tag's, a number's, or an object's blockHash or blockNumber (EIP-1898), on the main
/// chain, through history, as far back as this node keeps it.
fn state_at(api: &Api, block: &Value) -> Result<(Arc<State>, BlockHeader), RpcError> {
    let height = match block {
        Value::Null => None,
        Value::String(tag) => match tag.as_str() {
            "latest" | "pending" => None,
            "earliest" => Some(0),
            named @ ("safe" | "finalized") => Some(tagged(api, named)?.ok_or_else(|| RpcError::server(format!("this chain has no {named} block")))?),
            _ => Some(u64::try_from(number(block, "a block number")?).map_err(|_| RpcError::invalid("a block number past 64 bits"))?),
        },
        Value::Object(fields) => match (fields.get("blockHash"), fields.get("blockNumber")) {
            (Some(hash), _) => {
                let block = block_by_hash(api, hash)?.ok_or_else(|| RpcError::server("no block on the main chain by that hash"))?;

                Some(block.header.height)
            }
            (None, Some(number)) => return state_at(api, number),
            (None, None) => None,
        },
        _ => return Err(RpcError::invalid("a block is a tag, a number, or an object naming one")),
    };
    let tip = tip(&*api.db)?.header;

    match height {
        None => Ok((Arc::new(State::new(api.db.clone())), tip)),
        Some(height) if height > tip.height => Err(RpcError::server(format!("block {height} is past the tip, {}", tip.height))),
        Some(height) => {
            let sealed = stored_header_at(&*api.db, height)?.ok_or_else(|| RpcError::server(format!("no block at {height}")))?;
            let state = stored_state_at(api.db.clone(), &sealed.id())?
                .ok_or_else(|| RpcError::server(format!("this node no longer keeps the state at block {height}: ask an archive node")))?;

            Ok((state, sealed.header))
        }
    }
}

/// A call, as eth_call and eth_estimateGas take it.
#[derive(Clone)]
struct Call {
    from: [u8; 20],
    to: Option<[u8; 20]>,
    value: u256,
    input: Vec<u8>,
    gas: Option<u64>,
}

impl Call {
    fn read(value: &Value) -> Result<Call, RpcError> {
        let field = |name: &str| value.get(name).filter(|field| !field.is_null());
        let input = match field("input").or_else(|| field("data")) {
            Some(data) => bytes(data, "the input")?,
            None => Vec::new(),
        };

        Ok(Call {
            from: field("from").map(address).transpose()?.unwrap_or([0; 20]),
            to: field("to").map(address).transpose()?,
            value: field("value").map(|value| number(value, "the value")).transpose()?.unwrap_or(u256::ZERO),
            input,
            gas: field("gas").map(|gas| number(gas, "the gas")).transpose()?.map(|gas| u64::try_from(gas).unwrap_or(u64::MAX)),
        })
    }
}

/// Runs a call on a throwaway copy of a block's state, as the block after it would run it, as its
/// sender at its next nonce. Nothing it writes is kept.
async fn simulate(api: &Arc<Api>, call: Call, (at, header): (Arc<State>, BlockHeader)) -> Result<EvmRan, RpcError> {
    let api = api.clone();
    let chain = chain(&api)?;

    tokio::task::spawn_blocking(move || -> Result<EvmRan, RpcError> {
        let state = Arc::new(State::new(at));
        let sender = account_of(&state, call.from)?;
        let nonce = Accounts::new(&state).get(&sender)?.nonce;
        let fuel_per_gas = api.rules.fees.as_ref().map_or(1, |fees| fees.schedule().fuel_for(1).max(1));
        let cap = api.rules.limits.fuel / fuel_per_gas;
        let action = match call.to {
            None => EvmAction::Create { init: call.input, value: call.value },
            Some(to) => EvmAction::Call { to: Address::Evm(to), value: call.value, input: call.input },
        };
        let block = EvmBlock {
            height: header.height + 1,
            timestamp: header.timestamp + 1,
            producer: header.producer,
            chain_id: chain,
            names: api.rules.names.as_deref().cloned(),
            hash: api.rules.hash,
            vm: api.rules.vm,
        };

        let bridge = evm::Bridge {
            vm: api.vm.clone(),
            fuel_per_gas,
            stack: Vec::new(),
        };

        Ok(evm::run(&state, &sender, nonce, action, &[], call.gas.map_or(cap, |gas| gas.min(cap)), api.rules.limits.memory as u64, block, Some(&bridge))?)
    })
    .await
    .map_err(|err| RpcError::server(err.to_string()))?
}

/// What a call that failed answers: a revert with its reason and the bytes it returned, as
/// Ethereum's nodes give them, code 3; anything else, why.
fn failed(why: &str, reverted: &[u8]) -> RpcError {
    match why.strip_prefix("reverted: ") {
        Some(reason) => RpcError {
            code: 3,
            message: format!("execution reverted: {reason}"),
            data: Some(to_hex(reverted).into()),
        },
        None => RpcError::server(why),
    }
}

/// The gas a transaction of the call needs on this chain: its intrinsic cost, for its bytes, and
/// the EVM's gas as fuel, with a quarter more of what ran, as POST /offer sizes a limit.
fn estimate(api: &Api, call: &Call, evm_gas: u64) -> u64 {
    let Some(fees) = &api.fees else {
        return evm_gas + evm_gas / 4;
    };
    // A stand-in of its size, signed by any key: its bytes are what the intrinsic cost weighs.
    let stand_in = EthereumTx {
        kind: EthereumKind::DynamicFee,
        chain_id: u64::MAX >> 16,
        nonce: u64::from(u32::MAX),
        max_price: u256::from(u64::MAX),
        tip: u256::from(u64::MAX),
        gas_limit: u64::from(u32::MAX),
        to: call.to,
        value: call.value,
        input: call.input.clone(),
        access_list: Vec::new(),
        signature: [0; 64],
        parity: 0,
    }
    .signed(&[1; 32]);
    let probe = SignedTx::from_ethereum(stand_in.encode()).expect("a transaction just signed reads");
    let fuel = evm_gas.saturating_mul(fees.schedule().fuel_for(1).max(1));
    let used = fees.schedule().intrinsic(&probe).saturating_add(fees.schedule().for_fuel(fuel));

    fees.estimated(api.tx_fuel, &probe, used)
}

/// The fee history: each block's base price and how full it was, and the next block's price.
fn fee_history(api: &Api, count: &Value, newest: &Value, percentiles: &Value) -> Answer {
    let count = match count {
        Value::Number(count) => count.as_u64().unwrap_or(1),
        count => u64::try_from(number(count, "the block count")?).unwrap_or(1),
    }
    .clamp(1, api.tuned.rpc.fee_history.max(1));
    let tip = tip(&*api.db)?.header.height;
    let newest = match newest.as_str() {
        Some(named @ ("safe" | "finalized")) => tagged(api, named)?.unwrap_or(tip),
        Some("latest" | "pending") | None => tip,
        Some(_) => u64::try_from(number(newest, "the newest block")?).unwrap_or(tip).min(tip),
    };
    let oldest = newest.saturating_sub(count - 1);
    let limit = api.fees.as_ref().map_or(1, |fees| fees.block_gas_limit().max(1));
    let (mut prices, mut ratios, mut rewards) = (Vec::new(), Vec::new(), Vec::new());
    let wanted = percentiles.as_array().map_or(0, Vec::len);

    for height in oldest..=newest {
        let block = stored_block_at(&*api.db, height)?.ok_or_else(|| RpcError::server(format!("no block at {height}")))?;
        prices.push(quantity(block.header.base_price));
        ratios.push(json!(block.header.gas_used as f64 / limit as f64));
        rewards.push(json!(vec![quantity(u256::ZERO); wanted]));
    }

    prices.push(quantity(next_base(api)?));
    let mut answer = json!({ "oldestBlock": quantity(u256::from(oldest)), "baseFeePerGas": prices, "gasUsedRatio": ratios });

    if wanted > 0 {
        answer["reward"] = rewards.into();
    }

    Ok(answer)
}

/// The height a tag names on the main chain: latest and pending the tip; finalized and safe the
/// finalized and the justified checkpoints' blocks, on a chain with finality, and none on one
/// without, as an Ethereum node on proof of work names none: a client then counts depth.
fn tagged(api: &Api, tag: &str) -> Result<Option<u64>, RpcError> {
    let tip = tip(&*api.db)?.header.height;

    match tag {
        "safe" | "finalized" if api.finality.is_none() => Ok(None),
        "safe" | "finalized" => {
            let (justified, finalized) = crate::chain::stored_finality(&*api.db)?;
            let checkpoint = if tag == "safe" { justified } else { finalized };

            match checkpoint {
                // Before anything past genesis is.
                None => Ok(Some(0)),
                Some(checkpoint) => Ok(Some(crate::chain::stored_header(&*api.db, &checkpoint.block)?.map_or(0, |sealed| sealed.header.height))),
            }
        }
        _ => Ok(Some(tip)),
    }
}

/// The block a tag or number names on the main chain: latest and pending the tip, finalized and
/// safe as tagged says.
fn block_at(api: &Api, tag: &Value) -> Result<Option<Block>, RpcError> {
    let height = match tag.as_str() {
        Some(named @ ("safe" | "finalized")) => match tagged(api, named)? {
            Some(height) => height,
            None => return Ok(None),
        },
        Some("latest" | "pending") | None => tip(&*api.db)?.header.height,
        Some("earliest") => 0,
        Some(_) => match u64::try_from(number(tag, "a block number")?) {
            Ok(height) => height,
            Err(_) => return Ok(None),
        },
    };

    Ok(stored_block_at(&*api.db, height)?)
}

fn block_by_hash(api: &Api, hash: &Value) -> Result<Option<Block>, RpcError> {
    let Some(id) = Hash::from_bytes(api.hash, &bytes(hash, "a block hash")?) else {
        return Ok(None);
    };

    Ok(stored_block(&*api.db, &id)?.filter(|block| stored_block_at(&*api.db, block.header.height).ok().flatten().is_some_and(|main| main.header.id() == id)))
}

/// A block's Ethereum-signed transactions, in order, with their place among all of its
/// transactions.
fn ethereum_txs(block: &Block) -> impl Iterator<Item = (usize, &SignedTx, &[u8])> {
    block.txs.iter().enumerate().filter_map(|(at, signed)| match &signed.tx {
        Tx::Ethereum { raw } => Some((at, signed, raw.as_slice())),
        _ => None,
    })
}

/// An address as JSON-RPC writes one: 0x and 20 bytes, the account's EVM address.
fn evm_address(address: &Address) -> Value {
    to_hex(&evm::evm_alias(address)).into()
}

fn block_json(api: &Api, block: &Block, full: bool) -> Value {
    let header = &block.header;
    let id = header.id();
    let limit = api.fees.as_ref().map_or(30_000_000, |fees| fees.block_gas_limit());
    let txs: Vec<Value> = ethereum_txs(block)
        .enumerate()
        .map(|(index, (_, signed, raw))| match full {
            true => Found { block: block.clone(), at: index, signed: signed.clone(), raw: raw.to_vec(), receipt: None }.tx_json(api),
            false => to_hex(&tx_hash(raw)).into(),
        })
        .collect();

    json!({
        "number": quantity(u256::from(header.height)),
        "hash": to_hex(&id.to_bytes()),
        "parentHash": to_hex(&header.parent.to_bytes()),
        "nonce": "0x0000000000000000",
        "sha3Uncles": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
        "logsBloom": to_hex(&[0; 256]),
        "transactionsRoot": to_hex(&header.tx_root.to_bytes()),
        "stateRoot": to_hex(&header.state_root.to_bytes()),
        "receiptsRoot": to_hex(&header.receipts_root.to_bytes()),
        "miner": evm_address(&header.producer),
        "difficulty": quantity(u256::from(header.difficulty)),
        "totalDifficulty": "0x0",
        "extraData": "0x",
        "size": quantity(u256::from(wincode::serialized_size(block).unwrap_or(0))),
        "gasLimit": quantity(u256::from(limit)),
        "gasUsed": quantity(u256::from(header.gas_used)),
        "timestamp": quantity(u256::from(header.timestamp / 1_000)),
        "baseFeePerGas": quantity(header.base_price),
        "mixHash": to_hex(&[0; 32]),
        "transactions": txs,
        "uncles": [],
    })
}

/// An Ethereum-signed transaction found on the main chain: its block, its place among the block's
/// Ethereum-signed ones, and what it did.
struct Found {
    block: Block,
    at: usize,
    signed: SignedTx,
    raw: Vec<u8>,
    receipt: Option<TxReceipt>,
}

/// A transaction still waiting in the node's mempool, by its hash: with no block, as Ethereum's
/// nodes show a pending one. Null when none waits by that hash.
async fn pending(api: &Api, hash: &Value) -> Answer {
    let hash: [u8; 32] = bytes(hash, "a transaction hash")?.try_into().map_err(|_| RpcError::invalid("a transaction hash is 32 bytes"))?;
    let Some(node) = &api.node else {
        return Ok(Value::Null);
    };
    let answer = node.mempool(crate::node::MempoolQuery::EthereumByHash(hash)).await.map_err(|err| RpcError::server(err.to_string()))?;

    Ok(match answer {
        crate::node::MempoolAnswer::Ethereum(Some(signed)) => match &signed.tx {
            Tx::Ethereum { raw } => ethereum_json(&signed, raw, None),
            _ => Value::Null,
        },
        _ => Value::Null,
    })
}

/// The transaction a keccak hash names, once it landed on the main chain.
fn found(api: &Api, hash: &Value) -> Result<Option<Found>, RpcError> {
    let hash: [u8; 32] = bytes(hash, "a transaction hash")?.try_into().map_err(|_| RpcError::invalid("a transaction hash is 32 bytes"))?;
    let Some(id) = stored_ethereum_tx(&*api.db, &hash)? else {
        return Ok(None);
    };
    let Some(receipt) = stored_receipt(&*api.db, &id)? else {
        return Ok(None);
    };
    let Some(block) = stored_block(&*api.db, &receipt.block)? else {
        return Ok(None);
    };

    // A receipt left from a branch that lost: the transaction is not on the main chain now.
    if stored_block_at(&*api.db, block.header.height)?.is_none_or(|main| main.header.id() != receipt.block) {
        return Ok(None);
    }

    let Some(signed) = block.txs.get(receipt.index as usize).cloned() else {
        return Ok(None);
    };
    let Tx::Ethereum { raw } = &signed.tx else {
        return Ok(None);
    };
    let raw = raw.clone();
    let at = ethereum_txs(&block).position(|(index, _, _)| index == receipt.index as usize).unwrap_or(0);

    Ok(Some(Found { block, at, signed, raw, receipt: Some(receipt) }))
}

impl Found {
    fn tx_json(&self, _api: &Api) -> Value {
        ethereum_json(&self.signed, &self.raw, Some((&self.block.header, self.at)))
    }
}

/// An Ethereum-signed transaction as JSON-RPC shows one: in a block, at its place among the
/// block's Ethereum-signed ones, or still waiting, with no block.
fn ethereum_json(signed: &SignedTx, raw: &[u8], place: Option<(&crate::structs::block::BlockHeader, usize)>) -> Value {
    let Ok(ethereum) = decode(raw) else {
        return Value::Null;
    };
    let mut value = json!({
        "hash": to_hex(&tx_hash(raw)),
        "nonce": quantity(u256::from(ethereum.nonce)),
        "blockHash": place.map(|(header, _)| to_hex(&header.id().to_bytes())),
        "blockNumber": place.map(|(header, _)| quantity(u256::from(header.height))),
        "transactionIndex": place.map(|(_, at)| quantity(u256::from(at as u64))),
        "from": evm_address(&signed.sender()),
        "to": ethereum.to.map(|to| to_hex(&to)),
        "value": quantity(ethereum.value),
        "gas": quantity(u256::from(ethereum.gas_limit)),
        "input": to_hex(&ethereum.input),
        "type": quantity(u256::from(ethereum.kind.number())),
        "chainId": quantity(u256::from(ethereum.chain_id)),
        "r": quantity(u256::from_be_bytes(ethereum.signature[..32].try_into().expect("32 bytes"))),
        "s": quantity(u256::from_be_bytes(ethereum.signature[32..].try_into().expect("32 bytes"))),
    });

    match ethereum.kind {
        EthereumKind::Legacy => {
            value["gasPrice"] = quantity(ethereum.max_price);
            value["v"] = quantity(u256::from(ethereum.chain_id * 2 + 35 + u64::from(ethereum.parity)));
        }
        kind => {
            value["v"] = quantity(u256::from(ethereum.parity));
            value["yParity"] = quantity(u256::from(ethereum.parity));
            value["accessList"] = ethereum
                .access_list
                .iter()
                .map(|item| json!({ "address": to_hex(&item.address), "storageKeys": item.slots.iter().map(|slot| to_hex(slot)).collect::<Vec<_>>() }))
                .collect::<Vec<_>>()
                .into();

            match kind {
                EthereumKind::DynamicFee => {
                    value["maxFeePerGas"] = quantity(ethereum.max_price);
                    value["maxPriorityFeePerGas"] = quantity(ethereum.tip);
                    // What it pays: its block's base price and its tip, at most its max; waiting, its max.
                    value["gasPrice"] = quantity(place.map_or(ethereum.max_price, |(header, _)| ethereum.max_price.min(header.base_price + ethereum.tip)));
                }
                _ => value["gasPrice"] = quantity(ethereum.max_price),
            }
        }
    }

    value
}

impl Found {
    fn receipt_json(&self, api: &Api) -> Value {
        let Some(receipt) = &self.receipt else {
            return Value::Null;
        };
        let Ok(ethereum) = decode(&self.raw) else {
            return Value::Null;
        };
        let header = &self.block.header;
        let block_hash = to_hex(&header.id().to_bytes());
        let hash = to_hex(&tx_hash(&self.raw));
        // Gas and logs of the block's Ethereum-signed transactions before this one.
        let (mut cumulative, mut log_index) = (0u64, 0usize);

        for (index, signed, _) in ethereum_txs(&self.block) {
            if index >= receipt.index as usize {
                break;
            }

            if let Ok(Some(before)) = stored_receipt(&*api.db, &signed.id(api.hash)) {
                cumulative += before.gas_used;
                log_index += evm_logs(&before).count();
            }
        }

        let logs: Vec<Value> = evm_logs(receipt)
            .enumerate()
            .map(|(offset, (contract, topics, data))| {
                json!({
                    "address": to_hex(&contract),
                    "topics": topics.iter().map(|topic| to_hex(topic)).collect::<Vec<_>>(),
                    "data": to_hex(data),
                    "blockNumber": quantity(u256::from(header.height)),
                    "blockHash": block_hash,
                    "transactionHash": hash,
                    "transactionIndex": quantity(u256::from(self.at as u64)),
                    "logIndex": quantity(u256::from((log_index + offset) as u64)),
                    "removed": false,
                })
            })
            .collect();
        let price = match receipt.gas_used {
            0 => u256::ZERO,
            used => receipt.fee / u256::from(used),
        };

        json!({
            "transactionHash": hash,
            "transactionIndex": quantity(u256::from(self.at as u64)),
            "blockHash": block_hash,
            "blockNumber": quantity(u256::from(header.height)),
            "from": evm_address(&self.signed.sender()),
            "to": ethereum.to.map(|to| to_hex(&to)),
            "contractAddress": match &receipt.status {
                TxStatus::Deployed(address) => evm_address(address),
                _ => Value::Null,
            },
            "gasUsed": quantity(u256::from(receipt.gas_used)),
            "cumulativeGasUsed": quantity(u256::from(cumulative + receipt.gas_used)),
            "effectiveGasPrice": quantity(price),
            "status": quantity(u256::from(u64::from(!receipt.status.failed()))),
            "type": quantity(u256::from(ethereum.kind.number())),
            "logs": logs,
            "logsBloom": to_hex(&[0; 256]),
        })
    }
}

/// A receipt's EVM logs: each contract's 20 bytes, its topics and its data.
fn evm_logs(receipt: &TxReceipt) -> impl Iterator<Item = ([u8; 20], Vec<Vec<u8>>, &[u8])> {
    receipt.events.iter().filter_map(|event| {
        let Address::Evm(contract) = event.contract else {
            return None;
        };

        if event.name != LOG {
            return None;
        }

        let values: Vec<&[u8]> = event
            .data
            .iter()
            .map(|value| match value {
                TxCallArgumentType::Bytes(bytes) => Some(bytes.as_slice()),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let (data, topics) = values.split_last()?;

        Some((contract, topics.iter().map(|topic| topic.to_vec()).collect(), *data))
    })
}

/// eth_getLogs: the EVM logs of a range of main-chain blocks, or of one block by hash, that come
/// from an address of those asked, if any, with the topics asked at each place, if any.
fn logs(api: &Api, filter: &Value) -> Answer {
    let tip = tip(&*api.db)?.header.height;
    let height = |name: &str, default: u64| -> Result<u64, RpcError> {
        match filter.get(name) {
            None | Some(Value::Null) => Ok(default),
            Some(Value::String(tag)) if matches!(tag.as_str(), "safe" | "finalized") => Ok(tagged(api, tag)?.unwrap_or(tip)),
            Some(Value::String(tag)) if matches!(tag.as_str(), "latest" | "pending") => Ok(tip),
            Some(Value::String(tag)) if tag == "earliest" => Ok(0),
            Some(value) => Ok(u64::try_from(number(value, name)?).unwrap_or(u64::MAX).min(tip)),
        }
    };
    // The filter first, so a bad one costs no block read: sets, so each log is matched at once.
    let addresses: HashSet<[u8; 20]> = match filter.get("address") {
        None | Some(Value::Null) => HashSet::new(),
        Some(Value::Array(list)) if list.len() > api.tuned.rpc.log_addresses => {
            return Err(RpcError::invalid(format!("a filter names at most {} addresses", api.tuned.rpc.log_addresses)));
        }
        Some(Value::Array(list)) => list.iter().map(address).collect::<Result<_, _>>()?,
        Some(one) => HashSet::from([address(one)?]),
    };
    // At each place, the topics that match it; None matches any.
    let topics: Vec<Option<HashSet<Vec<u8>>>> = match filter.get("topics") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(places)) if places.len() > LOG_TOPIC_PLACES => {
            return Err(RpcError::invalid(format!("a log has topics at {LOG_TOPIC_PLACES} places at most")));
        }
        Some(Value::Array(places)) => places
            .iter()
            .map(|place| match place {
                Value::Null => Ok(None),
                Value::Array(any) if any.len() > api.tuned.rpc.log_topics => Err(RpcError::invalid(format!("a filter names at most {} topics at a place", api.tuned.rpc.log_topics))),
                Value::Array(any) => any.iter().map(|topic| bytes(topic, "a topic")).collect::<Result<_, _>>().map(Some),
                one => bytes(one, "a topic").map(|topic| Some(HashSet::from([topic]))),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(RpcError::invalid("topics is a list")),
    };
    // The blocks to look through, read one at a time as they are: a hash, or a range of heights.
    let (by_hash, range) = match filter.get("blockHash") {
        Some(hash) => (block_by_hash(api, hash)?, None),
        None => {
            let (from, to) = (height("fromBlock", tip)?, height("toBlock", tip)?);

            if to < from {
                return Ok(json!([]));
            }

            if to - from >= api.tuned.rpc.log_blocks {
                return Err(RpcError::server(format!("ask for at most {} blocks of logs at once", api.tuned.rpc.log_blocks)));
            }

            (None, Some(from..=to))
        }
    };
    let mut heights = range.into_iter().flatten();
    let mut by_hash = by_hash.into_iter();
    let mut found = Vec::new();

    loop {
        let block = match (by_hash.next(), heights.next()) {
            (Some(block), _) => block,
            (None, Some(at)) => match stored_block_at(&*api.db, at)? {
                Some(block) => block,
                // A block missing from the range, as the tip moved back under a reorg: nothing.
                None => return Ok(json!([])),
            },
            (None, None) => break,
        };
        let header = &block.header;
        let mut log_index = 0usize;
        let mut ethereum_index = 0usize;

        for signed in &block.txs {
            let Some(receipt) = stored_receipt(&*api.db, &signed.id(api.hash))?.filter(|receipt| receipt.block == header.id()) else {
                continue;
            };
            let (hash, at) = match &signed.tx {
                Tx::Ethereum { raw } => {
                    ethereum_index += 1;

                    (tx_hash(raw).to_vec(), ethereum_index - 1)
                }
                _ => (signed.id(api.hash).to_bytes(), receipt.index as usize),
            };

            for (contract, log_topics, data) in evm_logs(&receipt) {
                let index = log_index;
                log_index += 1;

                if !addresses.is_empty() && !addresses.contains(&contract) {
                    continue;
                }

                let matches = topics.iter().enumerate().all(|(place, wanted)| match wanted {
                    None => true,
                    Some(any) => log_topics.get(place).is_some_and(|topic| any.contains(topic)),
                });

                if !matches {
                    continue;
                }

                found.push(json!({
                    "address": to_hex(&contract),
                    "topics": log_topics.iter().map(|topic| to_hex(topic)).collect::<Vec<_>>(),
                    "data": to_hex(data),
                    "blockNumber": quantity(u256::from(header.height)),
                    "blockHash": to_hex(&header.id().to_bytes()),
                    "transactionHash": to_hex(&hash),
                    "transactionIndex": quantity(u256::from(at as u64)),
                    "logIndex": quantity(u256::from(index as u64)),
                    "removed": false,
                }));
            }
        }
    }

    Ok(found.into())
}
