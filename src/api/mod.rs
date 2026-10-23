//! An HTTP API over the committed chain, answering in JSON. It runs on a thread of its own and
//! reads the database directly, so a slow client never holds up the node loop. The one write,
//! POST /txs, hands a signed transaction to the node through a Submitter.

use {
    crate::chain::{
        FinalityConfig, Fees, Genesis, stored_block, stored_block_at, stored_evidence, stored_finality, stored_receipt, stored_tip,
        stored_work,
    },
    crate::crypto::keypair::PublicKey,
    crate::registry::signers::{SignerSet, Signers},
    crate::registry::validators::Validators,
    crate::structs::vote::Checkpoint,
    crate::db::{DbError, Read, State},
    crate::mempool::MempoolError,
    crate::node::{SubmitError, Submitter},
    crate::registry::{Accounts, Contracts},
    crate::structs::address::Address,
    crate::structs::block::Block,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::from_hex,
    crate::structs::tx::SignedTx,
    axum::body::Bytes,
    axum::extract::{DefaultBodyLimit, Path, Query, State as Shared},
    axum::http::StatusCode,
    axum::response::{IntoResponse, Response},
    axum::routing::{get, post},
    axum::{Json, Router},
    r#macro::document,
    serde_json::{Value, json},
    std::collections::HashMap,
    std::net::SocketAddr,
    std::sync::Arc,
};

mod json;

/// How many blocks a listing returns when the client does not say.
const DEFAULT_PAGE: u64 = 20;
/// The most blocks one listing returns.
const MAX_PAGE: u64 = 100;
/// The largest request body: room for the largest deploy, hex-encoded.
const MAX_BODY: usize = 2 * 1024 * 1024;

/// What every handler reads from.
#[derive(Clone)]
struct Api {
    db: Arc<dyn Read>,
    /// None when no node runs behind the API, so nothing can be submitted.
    submitter: Option<Submitter>,
    /// The chain's finality settings, to tell which epoch a height is in.
    finality: Option<Arc<FinalityConfig>>,
    /// The chain's hash, which ids are read and written with.
    hash: HashType,
    /// The chain's fees, to tell what the next block's base price is.
    fees: Option<Arc<Fees>>,
    /// The most fuel one transaction's contracts may burn, which clients size gas limits by.
    tx_fuel: u64,
}

#[document(
    summary = "Starts the HTTP API on a thread of its own. It answers GET requests in JSON from the committed chain and never writes.",
    category = "Node",
    params = {
        db = "The node's database. Start the API after Node::new, so genesis is in it.",
        submitter = "The running node's Submitter, for POST /txs; None to refuse submissions.",
        genesis = "The chain's genesis: its hash, and its finality settings if it has finality.",
        address = "Where to listen.",
    },
    returns = "The address it listens on, which tells you the port when address asked for port 0.",
    note = "Routes: GET /status, which includes the fees and the base price a transaction sent now must offer; GET /blocks, newest first, paged with ?before=height&limit=n (20 by default, at most 100); GET /blocks/{block}, where block is latest, a height or a 0x block id; GET /accounts/{address}; GET /contracts/{address}; GET /txs/{id}, a transaction's block, height, confirmations, whether it is final, result and events once it is on the main chain, 404 before; GET /validators, every registered validator and the current epoch's set; GET /validators/{voter}; GET /evidence, conflicting votes this node has seen, encoded for Tx::Slash; POST /txs with {\"tx\": \"0x…\"}, a SignedTx encoded as the chain encodes it, which answers 202 with its id. Addresses are written as their kind and hex, such as ed25519:0x3b6a…. Amounts, and integers that can pass 2^53, are decimal strings; hashes are 0x hex. Errors are {\"error\": message} with status 400, 404, 409 for a nonce already used or waiting, 500, or 503 when the node cannot take transactions.",
    see_also = "Node, Address, Submitter",
)]
pub fn start(
    db: Arc<dyn Read>,
    submitter: Option<Submitter>,
    genesis: &Genesis,
    address: SocketAddr,
) -> std::io::Result<SocketAddr> {
    let finality = genesis.finality.clone().map(Arc::new);
    let fees = genesis.fees.as_ref().map(|config| Arc::new(config.build()));
    let (hash, tx_fuel) = (genesis.hash, genesis.tx_limits.fuel);
    let listener = std::net::TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    let local = listener.local_addr()?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;

    std::thread::Builder::new()
        .name("http".to_string())
        .spawn(move || {
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(err) => return log::error!("[http] {err}"),
                };

                if let Err(err) = axum::serve(listener, router(Api {
                    db,
                    submitter,
                    finality,
                    hash,
                    fees,
                    tx_fuel,
                })).await {
                    log::error!("[http] {err}");
                }
            })
        })?;

    Ok(local)
}

fn router(api: Api) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/blocks", get(blocks))
        .route("/blocks/{block}", get(block))
        .route("/accounts/{address}", get(account))
        .route("/contracts/{address}", get(contract))
        .route("/txs", post(submit))
        .route("/txs/{id}", get(transaction))
        .route("/validators", get(validators))
        .route("/validators/{voter}", get(validator))
        .route("/signers", get(signers))
        .route("/evidence", get(evidence))
        .fallback(async || ApiError::NotFound("no such route".to_string()))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(api)
}

/// Why a request failed. Each becomes a status and {"error": message}.
enum ApiError {
    BadRequest(String),
    NotFound(String),
    /// The transaction's nonce is used or already waiting.
    Conflict(String),
    /// The node cannot take transactions right now.
    Unavailable(String),
    Db(DbError),
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> ApiError {
        ApiError::Db(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message),
            ApiError::Unavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, message),
            ApiError::Db(err) => {
                log::error!("[http] {err}");

                (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
            }
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}

type Answer = Result<Json<Value>, ApiError>;

async fn status(Shared(api): Shared<Api>) -> Answer {
    let db = api.db.clone();
    let genesis = at_height(&*db, 0)?;
    let tip = tip(&*db)?;

    let work = stored_work(&*db, &tip.id())?.unwrap_or_default();

    let finality = match &api.finality {
        Some(config) => {
            let (justified, finalized) = stored_finality(&*db)?;
            let start = Checkpoint {
                epoch: 0,
                block: genesis.id(),
            };

            json!({
                "type": config.name(),
                "epoch": config.epoch_of(tip.header.height),
                "checkpoint_interval": config.checkpoint_interval(),
                "justified": checkpoint_at(&*db, &justified.unwrap_or(start))?,
                "finalized": checkpoint_at(&*db, &finalized.unwrap_or(start))?,
            })
        }
        None => Value::Null,
    };

    // What a transaction sent now pays at least, and what it is weighed by.
    let fees = match &api.fees {
        Some(fees) => {
            let header = &tip.header;
            let next = fees.pricing().next_price(header.base_price, header.gas_used, fees.block_gas_limit());
            let mut value = crate::cli::genesis::fees_json(fees.config());
            value["base_price"] = next.to_string().into();
            value["tx_fuel"] = api.tx_fuel.into();

            value
        }
        None => Value::Null,
    };

    Ok(Json(json!({
        "genesis": genesis.id().to_string(),
        "tip": tip.id().to_string(),
        "height": tip.header.height,
        "timestamp": tip.header.timestamp,
        "difficulty": tip.header.difficulty.to_string(),
        "work": work.to_string(),
        "hash": api.hash.name(),
        "finality": finality,
        "fees": fees,
    })))
}

/// A checkpoint with its block's height.
fn checkpoint_at(db: &dyn Read, checkpoint: &Checkpoint) -> Result<Value, ApiError> {
    let mut value = json::checkpoint(checkpoint);
    value["height"] = stored_block(db, &checkpoint.block)?.map(|block| block.header.height).into();

    Ok(value)
}

/// The height of the finalized checkpoint, 0 before anything past genesis is.
fn finalized_height(db: &dyn Read) -> Result<u64, ApiError> {
    let (_, finalized) = stored_finality(db)?;
    let Some(finalized) = finalized else {
        return Ok(0);
    };

    Ok(stored_block(db, &finalized.block)?.map_or(0, |block| block.header.height))
}

/// Every bonded validator, and who votes in the tip's epoch: the bonded set under FFG, the fixed
/// signers under authority finality.
async fn validators(Shared(api): Shared<Api>) -> Answer {
    let Some(config) = &api.finality else {
        return Err(ApiError::NotFound("this chain has no finality".to_string()));
    };

    if !config.votes() {
        return Err(ApiError::NotFound(format!("{} finality has no validators", config.name())));
    }

    let db = api.db;
    let epoch = config.epoch_of(tip(&*db)?.header.height);
    let state = State::new(db);
    let validators = Validators::new(&state);
    let mut records = Vec::new();

    for voter in validators.voters()? {
        if let Some(record) = validators.get(&voter)? {
            records.push(json::validator(&voter, &record));
        }
    }

    let set = config.voters(&state, epoch)?.unwrap_or_default();

    Ok(Json(json!({
        "epoch": epoch,
        "validators": records,
        "active": set.members.iter().map(|member| json!({
            "voter": member.voter.to_string(),
            "account": member.account.to_string(),
            "weight": member.weight.to_string(),
        })).collect::<Vec<_>>(),
        "total_weight": set.total().to_string(),
    })))
}

async fn validator(Shared(api): Shared<Api>, Path(voter): Path<String>) -> Answer {
    let voter: PublicKey = voter
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("not a voting key: {voter}")))?;
    let state = State::new(api.db);
    let record = Validators::new(&state)
        .get(&voter)?
        .ok_or_else(|| ApiError::NotFound(format!("no validator has the voting key {voter}")))?;

    Ok(Json(json::validator(&voter, &record)))
}

/// Authority finality's signers: those voting in the tip's epoch, the set decided last, and the
/// approvals so far for changing it, by proposal.
async fn signers(Shared(api): Shared<Api>) -> Answer {
    let Some(config) = &api.finality else {
        return Err(ApiError::NotFound("this chain has no finality".to_string()));
    };

    if config.signer_rules().is_none() {
        return Err(ApiError::NotFound(format!("{} finality has no signers", config.name())));
    }

    let db = api.db;
    let epoch = config.epoch_of(tip(&*db)?.header.height);
    let state = State::new(db);
    let signers = Signers::new(&state);
    let decided = signers.decided()?;
    let mut proposals: Vec<(SignerSet, Vec<PublicKey>)> = Vec::new();

    for approval in signers.ballot()?.approvals {
        match proposals.iter_mut().find(|(proposal, _)| *proposal == approval.proposal) {
            Some((_, approvers)) => approvers.push(approval.signer),
            None => proposals.push((approval.proposal, vec![approval.signer])),
        }
    }

    Ok(Json(json!({
        "epoch": epoch,
        "active": signers.set(epoch)?.as_ref().map(json::signer_set),
        "version": decided.version,
        "decided": json::signer_set(&decided.set),
        "proposals": proposals.iter().map(|(proposal, approvers)| {
            let mut value = json::signer_set(proposal);
            value["approvals"] = approvers.iter().map(PublicKey::to_string).collect::<Vec<_>>().into();

            value
        }).collect::<Vec<_>>(),
    })))
}

/// Conflicting votes this node has seen, ready for Tx::Slash.
async fn evidence(Shared(api): Shared<Api>) -> Answer {
    let evidence = stored_evidence(&*api.db)?;

    Ok(Json(json!({
        "evidence": evidence.iter().map(|evidence| json!({
            "voter": evidence.first.vote.voter.to_string(),
            "first": json::vote(&evidence.first),
            "second": json::vote(&evidence.second),
        })).collect::<Vec<_>>(),
    })))
}

/// A transaction's receipt, once it is on the main chain.
async fn transaction(Shared(api): Shared<Api>, Path(id): Path<String>) -> Answer {
    let db = api.db.clone();
    let id = block_id(&id, api.hash)?;
    let receipt = stored_receipt(&*db, &id)?
        .ok_or_else(|| ApiError::NotFound(format!("transaction {id} is not on the main chain, or not yet")))?;
    let block = stored_block(&*db, &receipt.block)?
        .ok_or_else(|| ApiError::Db(DbError::Corrupt(format!("block {} is not stored", receipt.block))))?;
    let tip = tip(&*db)?.header.height;
    let confirmations = tip.saturating_sub(block.header.height) + 1;
    let mut value = json::receipt(&id.to_string(), &receipt, block.header.height, confirmations);
    value["final"] = (api.finality.is_some() && block.header.height <= finalized_height(&*db)?).into();

    Ok(Json(value))
}

/// Block summaries, newest first, below the height in ?before.
async fn blocks(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let top = tip(&*db)?.header.height + 1;
    let before = number(&query, "before")?.unwrap_or(top).min(top);
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_PAGE).min(MAX_PAGE);
    let first = before.saturating_sub(limit);
    let mut summaries = Vec::new();

    for height in (first..before).rev() {
        summaries.push(json::summary(&at_height(&*db, height)?));
    }

    Ok(Json(json!({
        "blocks": summaries,
        "next": (first > 0).then_some(first),
    })))
}

async fn block(Shared(api): Shared<Api>, Path(block): Path<String>) -> Answer {
    let (db, hash) = (api.db, api.hash);
    let found = if block == "latest" {
        tip(&*db)?
    } else if block.starts_with("0x") {
        let id = block_id(&block, hash)?;

        stored_block(&*db, &id)?.ok_or_else(|| ApiError::NotFound(format!("no block {id}")))?
    } else {
        let height = block
            .parse()
            .map_err(|_| ApiError::BadRequest(format!("not a height or a block id: {block}")))?;

        stored_block_at(&*db, height)?
            .ok_or_else(|| ApiError::NotFound(format!("no block at height {height}")))?
    };

    let id = found.id();
    let mut receipts = Vec::new();

    for tx in &found.txs {
        // A side-branch block's transactions may sit elsewhere on the main chain, or nowhere.
        receipts.push(stored_receipt(&*db, &tx.id(hash))?.filter(|receipt| receipt.block == id));
    }

    Ok(Json(json::block(&found, &receipts, hash)))
}

async fn account(Shared(api): Shared<Api>, Path(address): Path<String>) -> Answer {
    let db = api.db;
    let address = parse_address(&address)?;
    let account = Accounts::new(&State::new(db)).get(&address)?;

    Ok(Json(json::account(&address, &account)))
}

async fn contract(Shared(api): Shared<Api>, Path(address): Path<String>) -> Answer {
    let db = api.db;
    let address = parse_address(&address)?;
    let state = Arc::new(State::new(db));
    let record = Contracts::new(&state)
        .get(&address)?
        .ok_or_else(|| ApiError::NotFound(format!("no contract at {address}")))?;

    Ok(Json(json::contract(&address, &record)))
}

/// Takes a signed transaction for the node's mempool.
async fn submit(Shared(api): Shared<Api>, body: Bytes) -> Result<(StatusCode, Json<Value>), ApiError> {
    let Some(submitter) = api.submitter else {
        return Err(ApiError::Unavailable("this API has no node to submit to".to_string()));
    };
    let body: Value = serde_json::from_slice(&body).map_err(|err| ApiError::BadRequest(format!("not JSON: {err}")))?;
    let hex = body
        .get("tx")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("expected {\"tx\": \"0x…\"}".to_string()))?;
    let bytes = from_hex(hex).ok_or_else(|| ApiError::BadRequest("tx is not hex".to_string()))?;
    let tx: SignedTx = wincode::deserialize(&bytes)
        .map_err(|err| ApiError::BadRequest(format!("not a signed transaction: {err}")))?;

    // One encoding per transaction, so nothing can ride along after it.
    if wincode::serialize(&tx).ok().as_deref() != Some(&bytes[..]) {
        return Err(ApiError::BadRequest("tx has bytes past the transaction".to_string()));
    }

    match submitter.submit(tx).await {
        Ok(id) => Ok((StatusCode::ACCEPTED, Json(json!({ "id": id.to_string() })))),
        Err(SubmitError::Refused(err)) => Err(match err {
            MempoolError::Invalid(_)
            | MempoolError::BadGasLimit
            | MempoolError::PriceTooLow
            | MempoolError::CannotPay => ApiError::BadRequest(err.to_string()),
            MempoolError::Stale | MempoolError::Duplicate => ApiError::Conflict(err.to_string()),
            MempoolError::Full => ApiError::Unavailable(err.to_string()),
            MempoolError::Db(err) => ApiError::Db(err),
        }),
        Err(err @ SubmitError::Stopped) => Err(ApiError::Unavailable(err.to_string())),
    }
}

fn tip(db: &dyn Read) -> Result<Block, ApiError> {
    stored_tip(db)?.ok_or_else(|| ApiError::NotFound("the chain has no blocks yet".to_string()))
}

/// A block below the tip, which must be there.
fn at_height(db: &dyn Read, height: u64) -> Result<Block, ApiError> {
    stored_block_at(db, height)?
        .ok_or_else(|| ApiError::Db(DbError::Corrupt(format!("no block at height {height}"))))
}

fn number(query: &HashMap<String, String>, name: &str) -> Result<Option<u64>, ApiError> {
    query
        .get(name)
        .map(|text| {
            text.parse()
                .map_err(|_| ApiError::BadRequest(format!("{name} must be a whole number")))
        })
        .transpose()
}

/// A block or transaction id: both are BLAKE3.
/// A block or transaction id, in the chain's hash.
fn block_id(text: &str, hash: HashType) -> Result<Hash, ApiError> {
    from_hex(text)
        .and_then(|bytes| Hash::from_bytes(hash, &bytes))
        .ok_or_else(|| ApiError::BadRequest(format!("an id is 0x and {} hex digits: {text}", hash.digest_len() * 2)))
}

fn parse_address(text: &str) -> Result<Address, ApiError> {
    text.parse()
        .map_err(|err| ApiError::BadRequest(format!("{err}: {text}")))
}

#[cfg(test)]
mod tests;
