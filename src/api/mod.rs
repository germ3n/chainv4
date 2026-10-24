//! An HTTP API over the committed chain, answering in JSON. It runs on a thread of its own and
//! reads the database directly, so a slow client never holds up the node loop. The one write,
//! POST /txs, hands a signed transaction to the node through a NodeHandle.

use {
    crate::chain::fees::OfferChoice,
    crate::chain::{
        Coin, EventFilter, FinalityConfig, Fees, Genesis, stored_event_index, stored_events, stored_block, stored_block_at, stored_evidence, stored_finality, stored_on_main_chain,
        stored_receipt, stored_tip, stored_tips, stored_work,
    },
    crate::crypto::keypair::PublicKey,
    crate::registry::signers::{Approval, SignerSet, Signers},
    crate::registry::validators::Validators,
    crate::structs::vote::Checkpoint,
    crate::db::{Database, DbError, Read, State},
    crate::vm::{CallContext, Limits, Vm, VmError, selector},
    crate::mempool::MempoolError,
    crate::node::{NodeHandle, NodeReport, SubmitError},
    crate::registry::tokens::stored_tokens,
    crate::registry::{Accounts, Contracts, Tokens},
    crate::structs::address::Address,
    crate::structs::block::Block,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::{FeeOffer, SignedTx},
    crate::structs::u256::u256,
    axum::body::Bytes,
    axum::extract::{DefaultBodyLimit, Path, Query, Request, State as Shared},
    axum::http::{HeaderValue, Method, StatusCode, Uri, header},
    axum::middleware::{self, Next},
    axum::response::{IntoResponse, Response},
    axum::routing::{get, post},
    axum::{Json, Router},
    r#macro::document,
    serde_json::{Value, json},
    std::collections::{HashMap, HashSet},
    std::net::SocketAddr,
    std::path::PathBuf,
    std::sync::Arc,
};

#[cfg(feature = "stream")]
use axum::response::sse::{Event, KeepAlive, Sse};

mod json;

/// How many blocks a listing returns when the client does not say.
const DEFAULT_PAGE: u64 = 20;
/// The most blocks one listing returns.
const MAX_PAGE: u64 = 100;
/// How many side blocks /forks returns when the client does not say.
const DEFAULT_FORKS: u64 = 256;
/// The most side blocks /forks returns.
const MAX_FORKS: u64 = 1_000;
/// How many events /events returns when the client does not say.
const DEFAULT_EVENTS: u64 = 50;
/// The most events /events returns.
const MAX_EVENTS: u64 = 500;
/// How many tokens /tokens returns when the client does not say.
const DEFAULT_TOKENS: u64 = 50;
/// The most tokens /tokens returns.
const MAX_TOKENS: u64 = 500;
/// How many waiting transactions /mempool returns when the client does not say.
const DEFAULT_MEMPOOL: u64 = 100;
/// The most waiting transactions /mempool returns.
const MAX_MEMPOOL: u64 = 1_000;
/// The largest request body: room for the largest deploy, hex-encoded.
const MAX_BODY: usize = 2 * 1024 * 1024;

/// What every handler reads from.
#[derive(Clone)]
struct Api {
    /// Only ever read: a Database rather than a Read so listings can scan it.
    db: Arc<dyn Database>,
    /// None when no node runs behind the API, so nothing can be submitted.
    node: Option<NodeHandle>,
    /// The chain's finality settings, to tell which epoch a height is in.
    finality: Option<Arc<FinalityConfig>>,
    /// The chain's hash, which ids are read and written with.
    hash: HashType,
    /// The chain's fees, to tell what the next block's base price is.
    fees: Option<Arc<Fees>>,
    /// The most fuel one transaction's contracts may burn, which clients size gas limits by.
    tx_fuel: u64,
    /// The chain's coin, for clients to show balances by.
    coin: Arc<Coin>,
    /// The functions that make a contract a token on this chain.
    token_functions: Arc<[String]>,
    /// The chain's engines, for read-only calls.
    vm: Arc<Vm>,
    /// What one transaction's contracts may use, which bounds a read-only call too.
    limits: Limits,
    /// The browser origins allowed to call the API.
    cors: Arc<Vec<String>>,
    /// A built explorer web app, served from /.
    explorer: Option<Arc<PathBuf>>,
}

#[document(
    summary = "What an API serves besides the chain, and to which web pages.",
    category = "Node",
    see_also = "start",
)]
#[derive(Clone, Debug, Default)]
pub struct ApiOptions {
    #[document(summary = "Browser origins allowed to call the API from their pages, such as http://localhost:8081, or * for any. Empty lets only pages the API serves itself call it.")]
    pub cors: Vec<String>,
    #[document(summary = "A built explorer web app, trunk's dist directory, to serve from / on the API's own origin. Its files never shadow a route.")]
    pub explorer: Option<PathBuf>,
}

#[document(
    summary = "Starts the HTTP API on a thread of its own. It answers GET requests in JSON from the committed chain and never writes.",
    category = "Node",
    params = {
        db = "The node's database. Start the API after Node::new, so genesis is in it.",
        node = "The running node's handle, for POST /txs, /node, /mempool and /stream; None to serve only what the database holds.",
        genesis = "The chain's genesis: its hash, and its finality settings if it has finality.",
        address = "Where to listen.",
        options = "Who else it answers, and what it serves besides the chain.",
    },
    returns = "The address it listens on, which tells you the port when address asked for port 0.",
    note = "Routes: GET /status, which includes the fees and the base price a transaction sent now must offer; GET /blocks, newest first, paged with ?before=height&limit=n (20 by default, at most 100); GET /blocks/{block}, where block is latest, a height or a 0x block id; GET /accounts/{address}; GET /contracts/{address}; GET /txs/{id}, a transaction's block, height, confirmations, whether it is final, result and events once it is on the main chain, 404 before; GET /validators, every registered validator and the current epoch's set; GET /validators/{voter}; GET /evidence, conflicting votes this node has seen, encoded for Tx::Slash; POST /txs with {\"tx\": \"0x…\"}, a SignedTx encoded as the chain encodes it, which answers 202 with its id; POST /offer with the same body, signed with any offer, which answers the fee offer sure to cover it now; GET /forks, the side branches; GET /events, indexed events, on a node started with --index-events; GET /tokens, the token registry by symbol; GET /node and GET /mempool, from the running node; GET /stream, server-sent events from a node started with --stream. Addresses are written as their kind and hex, such as ed25519:0x3b6a…. Amounts, and integers that can pass 2^53, are decimal strings; hashes are 0x hex. Errors are {\"error\": message} with status 400, 404, 409 for a nonce already used or waiting, 500, or 503 when the node cannot take transactions.",
    see_also = "Node, Address, NodeHandle",
)]
pub fn start(
    db: Arc<dyn Database>,
    node: Option<NodeHandle>,
    genesis: &Genesis,
    address: SocketAddr,
    options: ApiOptions,
) -> std::io::Result<SocketAddr> {
    let finality = genesis.finality.clone().map(Arc::new);
    let fees = genesis.fees.as_ref().map(|config| Arc::new(config.build()));
    let (hash, tx_fuel, coin, limits) = (genesis.hash, genesis.tx_limits.fuel, genesis.coin.clone(), genesis.tx_limits);
    let token_functions = genesis.token_functions.clone().into();
    let vm = Arc::new(
        Vm::new(&genesis.engines, genesis.hash)
            .map_err(|engine| std::io::Error::other(format!("this node cannot run {} contracts", engine.name())))?,
    );
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
                    node,
                    finality,
                    hash,
                    fees,
                    tx_fuel,
                    coin: Arc::new(coin),
                    token_functions,
                    vm,
                    limits,
                    cors: Arc::new(options.cors),
                    explorer: options.explorer.map(Arc::new),
                })).await {
                    log::error!("[http] {err}");
                }
            })
        })?;

    Ok(local)
}

fn router(api: Api) -> Router {
    let cors = !api.cors.is_empty();
    let router = Router::new()
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
        .route("/forks", get(forks))
        .route("/events", get(events))
        .route("/tokens", get(tokens))
        .route("/node", get(node))
        .route("/mempool", get(mempool))
        .route("/offer", post(offer))
        .route("/call", post(call));
    #[cfg(feature = "stream")]
    let router = router.route("/stream", get(stream));
    let router = router.fallback(explorer).layer(DefaultBodyLimit::max(MAX_BODY));

    match cors {
        true => router.layer(middleware::from_fn_with_state(api.clone(), allow_origins)).with_state(api),
        false => router.with_state(api),
    }
}

/// Lets pages from the allowed origins read the API's answers, and answers their browsers'
/// preflight questions.
async fn allow_origins(Shared(api): Shared<Api>, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|origin| HeaderValue::from_bytes(origin.as_bytes()).ok())
        .filter(|origin| api.cors.iter().any(|allowed| allowed == "*" || origin.as_bytes() == allowed.as_bytes()));
    let preflight = request.method() == Method::OPTIONS && request.headers().contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
    let mut response = match (preflight, &origin) {
        (true, Some(_)) => StatusCode::NO_CONTENT.into_response(),
        _ => next.run(request).await,
    };

    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.append(header::VARY, HeaderValue::from_static("origin"));

        if preflight {
            headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST"));
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type"));
            headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        }
    }

    response
}

/// Serves the explorer web app's files for anything that is not a route: / is its index.html.
async fn explorer(Shared(api): Shared<Api>, uri: Uri) -> Result<Response, ApiError> {
    let missing = || ApiError::NotFound("no such route".to_string());
    let Some(root) = &api.explorer else {
        return Err(missing());
    };
    let wanted = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    let relative = std::path::Path::new(wanted);

    // Only plain names below the app's directory, never .. or an absolute path.
    if !relative.components().all(|part| matches!(part, std::path::Component::Normal(_))) {
        return Err(missing());
    }

    let path = root.join(relative);
    let bytes = std::fs::read(&path).map_err(|_| missing())?;
    let kind = match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    };

    Ok(([(header::CONTENT_TYPE, kind)], bytes).into_response())
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
        "coin": crate::cli::genesis::coin_json(&api.coin),
        "token_functions": &*api.token_functions,
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
/// approvals that still count for changing it, by proposal.
async fn signers(Shared(api): Shared<Api>) -> Answer {
    let Some(config) = &api.finality else {
        return Err(ApiError::NotFound("this chain has no finality".to_string()));
    };
    let Some(rules) = config.signer_rules() else {
        return Err(ApiError::NotFound(format!("{} finality has no signers", config.name())));
    };

    let db = api.db;
    let epoch = config.epoch_of(tip(&*db)?.header.height);
    let state = State::new(db);
    let signers = Signers::new(&state);
    let decided = signers.decided()?;
    let mut proposals: Vec<(SignerSet, Vec<Approval>)> = Vec::new();

    for approval in signers.ballot()?.counting(epoch, rules.approval_epochs).approvals {
        match proposals.iter_mut().find(|(proposal, _)| *proposal == approval.proposal) {
            Some((_, approvals)) => approvals.push(approval),
            None => proposals.push((approval.proposal.clone(), vec![approval])),
        }
    }

    Ok(Json(json!({
        "epoch": epoch,
        "active": signers.set(epoch)?.as_ref().map(json::signer_set),
        "version": decided.version,
        "decided": json::signer_set(&decided.set),
        "approval_epochs": rules.approval_epochs,
        "proposals": proposals.iter().map(|(proposal, approvals)| {
            let mut value = json::signer_set(proposal);
            value["approvals"] = approvals.iter().map(|approval| json!({
                "signer": approval.signer.to_string(),
                "epoch": approval.epoch,
                "counts_until": approval.epoch + rules.approval_epochs - 1,
            })).collect();

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

    let mut value = json::block(&found, &receipts, hash);
    value["main"] = stored_on_main_chain(&*db, &found)?.into();

    Ok(Json(value))
}

/// The side branches this node stores: every block off the main chain, newest first, from each
/// leaf back to where its branch meets the main chain.
async fn forks(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_FORKS).min(MAX_FORKS) as usize;
    let mut seen = HashSet::new();
    let mut side = Vec::new();

    'leaves: for leaf in stored_tips(&*db)? {
        let mut id = leaf;

        while seen.insert(id) {
            let Some(block) = stored_block(&*db, &id)? else {
                break;
            };

            if stored_on_main_chain(&*db, &block)? {
                break;
            }

            if side.len() == limit {
                break 'leaves;
            }

            id = block.header.parent;
            side.push(block);
        }
    }

    side.sort_by_key(|block| std::cmp::Reverse(block.header.height));
    let tips: Vec<String> = stored_tips(&*db)?.iter().map(Hash::to_string).collect();

    Ok(Json(json!({
        "blocks": side.iter().map(json::summary).collect::<Vec<_>>(),
        "tips": tips,
    })))
}

/// Indexed events, newest first: a contract's, a contract's of one name, or those that mention an
/// address, a page at a time.
async fn events(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let Some(index) = stored_event_index(&*db)? else {
        return Err(ApiError::NotFound("this node keeps no event index; start it with --index-events".to_string()));
    };
    let filter = match (query.get("contract"), query.get("name"), query.get("address")) {
        (Some(contract), None, None) => EventFilter::Contract(parse_address(contract)?),
        (Some(contract), Some(name), None) => EventFilter::Named(parse_address(contract)?, name.clone()),
        (None, None, Some(address)) => EventFilter::Mentions(parse_address(address)?),
        _ => {
            return Err(ApiError::BadRequest(
                "ask for a contract's events, with or without a name, or an address's".to_string(),
            ));
        }
    };
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_EVENTS).clamp(1, MAX_EVENTS) as usize;
    let cursor = query
        .get("cursor")
        .map(|cursor| from_hex(cursor).ok_or_else(|| ApiError::BadRequest("cursor is not hex".to_string())))
        .transpose()?;
    let (found, next) = stored_events(&*db, &filter, cursor.as_deref(), limit)?;
    let tip = tip(&*db)?.header.height;

    Ok(Json(json!({
        "events": found.iter().map(json::indexed_event).collect::<Vec<_>>(),
        "next": next.map(|next| to_hex(&next)),
        "indexed_from": index.from,
        "catching_up_to": (!index.caught_up(tip)).then(|| index.keep.lowest(tip)),
    })))
}

/// How the node behind the API is doing: its peers, what it heard and did, how syncing goes.
async fn node(Shared(api): Shared<Api>) -> Answer {
    let report = report(&api, 0).await?;

    Ok(Json(json::node(&report)))
}

/// The transactions waiting in the node's mempool, by sender and nonce.
async fn mempool(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_MEMPOOL).min(MAX_MEMPOOL) as usize;
    let report = report(&api, limit).await?;

    Ok(Json(json!({
        "count": report.waiting,
        "capacity": report.capacity,
        "txs": report.txs.iter().map(|tx| json::signed_tx(tx, api.hash)).collect::<Vec<_>>(),
    })))
}

async fn report(api: &Api, txs: usize) -> Result<NodeReport, ApiError> {
    let Some(node) = &api.node else {
        return Err(ApiError::Unavailable("this API has no node behind it".to_string()));
    };

    node.report(txs).await.map_err(|err| ApiError::Unavailable(err.to_string()))
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
    let token = Tokens::new(&state).get(&address)?;

    Ok(Json(json::contract(&address, &record, token.as_ref())))
}

/// The token registry by symbol, a page at a time: every token, or those whose symbol starts with
/// ?symbol=, ignoring ASCII case.
async fn tokens(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_TOKENS).clamp(1, MAX_TOKENS) as usize;
    let cursor = query
        .get("cursor")
        .map(|cursor| from_hex(cursor).ok_or_else(|| ApiError::BadRequest("cursor is not hex".to_string())))
        .transpose()?;
    let (found, next) = stored_tokens(&*api.db, query.get("symbol").map(String::as_str), cursor.as_deref(), limit)?;

    Ok(Json(json!({
        "tokens": found.iter().map(json::token).collect::<Vec<_>>(),
        "next": next.map(|next| to_hex(&next)),
    })))
}

/// Takes a signed transaction for the node's mempool.
async fn submit(Shared(api): Shared<Api>, body: Bytes) -> Result<(StatusCode, Json<Value>), ApiError> {
    let Some(node) = api.node else {
        return Err(ApiError::Unavailable("this API has no node to submit to".to_string()));
    };
    let tx = signed_tx(&json_body(&body)?)?;

    match node.submit(tx).await {
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

/// The fee offer sure to cover a transaction sent now, keeping what the body sets itself:
/// gas_limit, max_price and tip. The body's tx is the transaction signed with any offer, which
/// weighs what the real one will. Nothing, on a chain without fees.
async fn offer(Shared(api): Shared<Api>, body: Bytes) -> Answer {
    let body = json_body(&body)?;
    let probe = signed_tx(&body)?;
    let Some(fees) = &api.fees else {
        return Ok(Json(json::fee_offer(&FeeOffer::NONE)));
    };
    let amount = |name: &str| {
        body.get(name)
            .map(|value| {
                value
                    .as_str()
                    .and_then(|text| u256::from_str_radix(text, 10).ok())
                    .ok_or_else(|| ApiError::BadRequest(format!("{name} must be a whole amount, as a string")))
            })
            .transpose()
    };
    let gas_limit = body
        .get("gas_limit")
        .map(|value| value.as_u64().ok_or_else(|| ApiError::BadRequest("gas_limit must be a whole number".to_string())))
        .transpose()?;
    let chosen = OfferChoice {
        gas_limit,
        max_price: amount("max_price")?,
        tip: amount("tip")?,
    };
    let header = tip(&*api.db)?.header;
    let base = fees.pricing().next_price(header.base_price, header.gas_used, fees.block_gas_limit());

    Ok(Json(json::fee_offer(&fees.offer(base, api.tx_fuel, &probe, chosen))))
}

/// Runs a contract function as a transaction would call it, but on a throwaway copy of the
/// committed state: whatever it writes, sends or emits is dropped. For reading contracts, such as
/// a token's decimals or someone's balance.
async fn call(Shared(api): Shared<Api>, body: Bytes) -> Answer {
    let body = json_body(&body)?;
    let field = |name: &str| body.get(name).and_then(Value::as_str);
    let contract = parse_address(field("contract").ok_or_else(|| ApiError::BadRequest("contract is missing".to_string()))?)?;
    let function = field("function").ok_or_else(|| ApiError::BadRequest("function is missing".to_string()))?;
    let sender = field("sender").map(parse_address).transpose()?.unwrap_or(Address::Ed25519([0; 32]));
    let args = match body.get("args") {
        None => Vec::new(),
        Some(Value::Array(args)) => args
            .iter()
            .map(|arg| json::parse_argument(arg).map_err(ApiError::BadRequest))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(ApiError::BadRequest("args must be a list".to_string())),
    };
    let state = Arc::new(State::new(api.db.clone()));
    let record = Contracts::new(&state)
        .get(&contract)?
        .ok_or_else(|| ApiError::NotFound(format!("no contract at {contract}")))?;
    let context = CallContext {
        sender,
        contract,
        value: u256::ZERO,
    };
    let outcome = api.vm.call(&state, &record, context, &Hash::Xxh3_64(selector(function)), &args, api.limits);

    if let Err(VmError::Storage(err)) = &outcome.result {
        return Err(ApiError::Db(err.clone()));
    }

    Ok(Json(json::call(&outcome)))
}

/// What the node does, as server-sent events, for a node started with --stream. A client that
/// falls too far behind gets a lagged event saying how many it missed, and should read the
/// chain again.
#[cfg(feature = "stream")]
async fn stream(
    Shared(api): Shared<Api>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    use tokio_stream::{StreamExt, wrappers::BroadcastStream, wrappers::errors::BroadcastStreamRecvError};

    let events = api
        .node
        .as_ref()
        .and_then(NodeHandle::subscribe)
        .ok_or_else(|| ApiError::NotFound("this node does not stream; start it with --stream".to_string()))?;
    let hash = api.hash;
    let events = BroadcastStream::new(events).map(move |event| {
        let (name, data) = match event {
            Ok(event) => json::node_event(&event, hash),
            Err(BroadcastStreamRecvError::Lagged(missed)) => ("lagged", json!({ "missed": missed })),
        };

        Ok(Event::default().event(name).data(data.to_string()))
    });

    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

fn json_body(body: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(body).map_err(|err| ApiError::BadRequest(format!("not JSON: {err}")))
}

/// The signed transaction a body's tx holds, encoded as the chain encodes it.
fn signed_tx(body: &Value) -> Result<SignedTx, ApiError> {
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

    Ok(tx)
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
