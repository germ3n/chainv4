//! An HTTP API over the committed chain, answering in JSON. It runs on a thread of its own and
//! reads the database directly, so a slow client never holds up the node loop. The one write,
//! POST /txs, hands a signed transaction to the node through a NodeHandle.

use {
    crate::chain::fees::OfferChoice,
    crate::chain::{
        DryRun, Rules, dry_run, Coin, EventFilter, FinalityConfig, Fees, Genesis, IndexKind, REWARD_POOL, Reward, VoteRewardSource, stored_address_txs, stored_index, stored_events, stored_block, stored_block_at, stored_evidence, stored_finality, stored_on_main_chain,
        stored_receipt, stored_tip, stored_tips, stored_work, CommittedReceipt, stored_header, stored_header_at, stored_justification,
        stored_light_finality, stored_light_update, stored_state_at, stored_checkpoint_start, StartProblem,
    },
    crate::crypto::hashable::Hashable,
    crate::light::{LightParams, MAX_FINALITY_STEPS, MAX_LIGHT_HEADERS},
    crate::crypto::merkle,
    crate::crypto::keypair::PublicKey,
    crate::registry::signers::{Approval, SignerSet, Signers},
    crate::registry::changes::Changes,
    crate::registry::validators::Validators,
    crate::structs::vote::Checkpoint,
    crate::db::{Committed, Database, DbError, Read, STATE_COLUMNS, State, StateTree},
    crate::vm::{BlockAt, CallContext, Limits, Vm, VmError, selector},
    crate::mempool::MempoolError,
    crate::node::{NodeHandle, NodeReport, SubmitError},
    crate::registry::tokens::stored_tokens,
    crate::registry::collections::{Collections, stored_collections},
    crate::registry::{Accounts, Contracts, Tokens},
    crate::structs::address::Address,
    crate::structs::block::Block,
    crate::structs::hash::{Hash, HashType},
    crate::structs::hex::{from_hex, to_hex},
    crate::structs::tx::{FeeOffer, SignedTx, Tx},
    crate::structs::u256::u256,
    axum::body::Bytes,
    axum::extract::{DefaultBodyLimit, FromRequestParts, Path, Query, Request, State as Live},
    axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    axum::middleware::{self, Next},
    axum::response::{IntoResponse, Response},
    axum::routing::{get, post},
    axum::{Json, Router},
    r#macro::document,
    serde_json::{Value, json},
    std::collections::{HashMap, HashSet},
    std::net::{IpAddr, SocketAddr},
    std::path::PathBuf,
    std::sync::Arc,
};

#[cfg(feature = "stream")]
use axum::response::sse::{Event, KeepAlive, Sse};

#[cfg(feature = "faucet")]
mod faucet;
mod json;
#[cfg(feature = "ethereum")]
mod rpc;

#[cfg(feature = "faucet")]
pub use faucet::FaucetConfig;

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
/// How many transactions /accounts/{address}/txs returns when the client does not say.
const DEFAULT_ACCOUNT_TXS: u64 = 50;
/// The most transactions /accounts/{address}/txs returns.
const MAX_ACCOUNT_TXS: u64 = 200;
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

#[document(
    summary = "How many requests one client may have in flight at once unless --http-per-ip says otherwise: past it, 429. A client is its IP, or behind a proxy on the node's own network, the one the proxy says it forwards for.",
    category = "Node",
    see_also = "ApiOptions::per_ip",
)]
pub const DEFAULT_PER_IP: usize = 64;

/// What every handler reads from.
#[derive(Clone)]
struct Api {
    /// What a handler reads: a snapshot of the database taken as its request arrived, so every
    /// key it reads, the tip included, is from one commit. Committed rather than Read, so listings
    /// can scan it.
    db: Arc<dyn Committed>,
    /// The database itself, which snapshots are taken of. Never written here.
    live: Arc<dyn Database>,
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
    /// Genesis's federation, which stands until governance decides another.
    federation: Option<Arc<crate::registry::federation::Federation>>,
    /// The functions that make a contract a token on this chain.
    token_functions: Arc<[String]>,
    /// The chain's engines, for read-only calls.
    vm: Arc<Vm>,
    /// What one transaction's contracts may use, which bounds a read-only call too.
    limits: Limits,
    /// The browser origins allowed to call the API.
    cors: Arc<Vec<String>>,
    /// A built explorer web app, served from /, and a built wallet, from /wallet/.
    explorer: Option<Arc<PathBuf>>,
    wallet: Option<Arc<PathBuf>>,
    /// What light clients need to follow the chain.
    light: Arc<LightParams>,
    /// The rules transactions run under, for dry runs.
    rules: Arc<Rules>,
    /// The most bytes a block's transactions may take, if genesis bounds them.
    max_block_bytes: Option<u64>,
    /// What each block mints for its producer, to show with the block.
    reward: Reward,
    /// Where vote rewards come from, and so whether the pool is worth showing.
    vote_rewards: VoteRewardSource,
    /// Genesis's bootnodes, which stand until governance decides others.
    genesis_bootnodes: Arc<Vec<String>>,
    /// The faucet, on a node that runs one.
    #[cfg(feature = "faucet")]
    faucet: Option<Arc<faucet::Faucet>>,
    /// The swap contract the node tells clients of.
    #[cfg(feature = "swap")]
    swap_contract: Option<Address>,
    /// The DEX, and the EVM DEX's router, the node tells clients of.
    #[cfg(feature = "dex")]
    dex_contract: Option<Address>,
    #[cfg(feature = "dex")]
    dex_router: Option<[u8; 20]>,
    /// Requests in flight, by client.
    in_flight: Arc<InFlight>,
}

/// Requests in flight, by client, and how many each may have.
struct InFlight {
    per_ip: usize,
    counts: std::sync::Mutex<HashMap<IpAddr, usize>>,
}

/// One request counted in: counted out as it drops, answered or not.
struct Counted {
    in_flight: Arc<InFlight>,
    ip: IpAddr,
}

impl InFlight {
    /// Counts a request in, unless its client already has as many as it may.
    fn enter(in_flight: &Arc<InFlight>, ip: IpAddr) -> Option<Counted> {
        let mut counts = in_flight.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = counts.entry(ip).or_insert(0);

        if *count >= in_flight.per_ip {
            return None;
        }

        *count += 1;

        Some(Counted { in_flight: in_flight.clone(), ip })
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        let mut counts = self.in_flight.counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(count) = counts.get_mut(&self.ip) {
            *count -= 1;

            if *count == 0 {
                counts.remove(&self.ip);
            }
        }
    }
}

/// Who asks: the connection's IP, or behind a proxy on this machine or its private network, the
/// last one X-Forwarded-For names, which the proxy added itself.
fn asker(peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    let local = match peer.ip() {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    };
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit(',').next())
        .and_then(|last| last.trim().parse().ok());

    match (local, forwarded) {
        (true, Some(ip)) => ip,
        _ => peer.ip(),
    }
}

/// Turns away a client with too many requests in flight already, 429, so one cannot keep the rest
/// waiting. A stream, open for as long as the client listens, is not counted.
async fn per_client(Live(api): Live<Api>, axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>, request: Request, next: Next) -> Response {
    if request.uri().path() == "/stream" {
        return next.run(request).await;
    }

    match InFlight::enter(&api.in_flight, asker(peer, request.headers())) {
        Some(_counted) => next.run(request).await,
        None => (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "too many requests at once from you: wait for some to answer" }))).into_response(),
    }
}

/// The API as a handler sees it: its database a snapshot taken as the request arrived, never
/// moving under it however many keys it reads. Middleware, which reads nothing, takes the API as
/// it is (Live).
struct Shared<T>(T);

impl FromRequestParts<Api> for Shared<Api> {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(_: &mut axum::http::request::Parts, api: &Api) -> Result<Shared<Api>, std::convert::Infallible> {
        Ok(Shared(Api { db: api.live.snapshot(), ..api.clone() }))
    }
}

impl Api {
    /// The chain's bootnodes now: the ones governance decided last, or genesis's.
    fn bootnodes(&self, state: &State) -> Result<Vec<String>, ApiError> {
        Ok(Changes::new(state).bootnodes()?.unwrap_or_else(|| (*self.genesis_bootnodes).clone()))
    }
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
    #[document(summary = "A built wallet web app, trunk's dist directory, to serve from /wallet/ on the API's own origin, so it talks to the node that served it.")]
    pub wallet: Option<PathBuf>,
    #[document(summary = "A faucet to run at POST /faucet, for a dev or test chain; None for none.")]
    #[cfg(feature = "faucet")]
    pub faucet: Option<FaucetConfig>,
    #[document(summary = "The atomic swap contract on this chain, contracts/lua/swap.lua as deployed here, which GET /status names for the wallet and the CLI; None for none.")]
    #[cfg(feature = "swap")]
    pub swap_contract: Option<Address>,
    #[document(summary = "The DEX on this chain, contracts/lua/dex.lua as deployed here, which GET /status names for the wallet; None for none.")]
    #[cfg(feature = "dex")]
    pub dex_contract: Option<Address>,
    #[document(summary = "The EVM DEX's router on this chain, by its 20 bytes, which GET /status names; None for none.")]
    #[cfg(feature = "dex")]
    pub dex_router: Option<[u8; 20]>,
    #[document(summary = "The threads the API answers on: as many as the machine has cores when 0.")]
    pub threads: usize,
    #[document(summary = "How many requests one client may have in flight at once; DEFAULT_PER_IP when 0.")]
    pub per_ip: usize,
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
    note = "Routes: GET /status, which includes the fees and the base price a transaction sent now must offer; GET /blocks, newest first, paged with ?before=height&limit=n (20 by default, at most 100); GET /blocks/{block}, where block is latest, a height or a 0x block id; GET /accounts/{address}; GET /contracts/{address}; GET /txs/{id}, a transaction's block, height, confirmations, whether it is final, result and events once it is on the main chain, 404 before; GET /validators, every registered validator and the current epoch's set; GET /validators/{voter}; GET /evidence, conflicting votes this node has seen, encoded for Tx::Slash; POST /txs with {\"tx\": \"0x…\"}, a SignedTx encoded as the chain encodes it, which answers 202 with its id; POST /offer with the same body, signed with any offer, which answers the fee offer sure to cover it now; GET /forks, the side branches; GET /events, indexed events, on a node started with --index-events; GET /tokens, the token registry by symbol; GET /collections, the NFT and multi-token collections by symbol; GET /node and GET /mempool, from the running node; GET /stream, server-sent events from a node started with --stream; GET /faucet and POST /faucet, on a node run with --faucet-key. Addresses are written as their kind and hex, such as ed25519:0x3b6a…. Amounts, and integers that can pass 2^53, are decimal strings; hashes are 0x hex. Errors are {\"error\": message} with status 400, 404, 409 for a nonce already used or waiting, 429 for a faucet asked again too soon, 500, or 503 when the node cannot take transactions.",
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
    let light = Arc::new(genesis.light_params());
    let fees = genesis.fees.as_ref().map(|config| Arc::new(config.build()));
    let (hash, tx_fuel, coin, limits) = (genesis.hash, genesis.tx_limits.fuel, genesis.coin.clone(), genesis.tx_limits);
    let token_functions = genesis.token_functions.clone().into();
    let federation = genesis.federation.clone().map(Arc::new);
    let (rules, max_block_bytes, reward, vote_rewards) = (Arc::new(genesis.rules()), genesis.max_block_bytes, genesis.reward, genesis.vote_rewards());
    let genesis_bootnodes = Arc::new(genesis.bootnodes.clone());
    let vm = Arc::new(
        Vm::new(&genesis.engines, genesis.hash)
            .map_err(|engine| std::io::Error::other(format!("this node cannot run {} contracts", engine.name())))?
            .with_names(genesis.names.clone())
            .with_federation(genesis.federation.clone())
            .with_chain_id(crate::crypto::ethereum::chain_id(&genesis.block().id())),
    );
    let listener = std::net::TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    let local = listener.local_addr()?;
    // Requests answered side by side, a thread a core unless told: a slow one holds up no other.
    let threads = match options.threads {
        0 => std::thread::available_parallelism().map_or(2, usize::from),
        threads => threads,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).thread_name("http").enable_all().build()?;
    let per_ip = match options.per_ip {
        0 => DEFAULT_PER_IP,
        per_ip => per_ip,
    };

    std::thread::Builder::new()
        .name("http".to_string())
        .spawn(move || {
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(err) => return log::error!("[http] {err}"),
                };

                if let Err(err) = axum::serve(listener, router(Api {
                    db: db.clone(),
                    live: db,
                    node,
                    finality,
                    hash,
                    fees,
                    tx_fuel,
                    coin: Arc::new(coin),
                    token_functions,
                    federation,
                    vm,
                    limits,
                    cors: Arc::new(options.cors),
                    explorer: options.explorer.map(Arc::new),
                    wallet: options.wallet.map(Arc::new),
                    light,
                    rules,
                    max_block_bytes,
                    reward,
                    vote_rewards,
                    genesis_bootnodes,
                    #[cfg(feature = "faucet")]
                    faucet: options.faucet.map(|config| Arc::new(faucet::Faucet::new(config))),
                    #[cfg(feature = "swap")]
                    swap_contract: options.swap_contract,
                    #[cfg(feature = "dex")]
                    dex_contract: options.dex_contract,
                    #[cfg(feature = "dex")]
                    dex_router: options.dex_router,
                    in_flight: Arc::new(InFlight { per_ip, counts: Default::default() }),
                })
                // Who connects, for the faucet's limits by IP.
                .into_make_service_with_connect_info::<SocketAddr>())
                .await
                {
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
        .route("/accounts/{address}/txs", get(account_txs))
        .route("/contracts/{address}", get(contract))
        .route("/light", get(light))
        .route("/light/updates/{block}", get(light_update))
        .route("/light/finality", get(light_finality))
        .route("/light/checkpoint", get(light_checkpoint_final))
        .route("/light/checkpoint/{block}", get(light_checkpoint))
        .route("/headers", get(headers))
        .route("/justifications", get(justifications))
        .route("/proofs/receipts/{id}", get(receipt_proof))
        .route("/proofs/{column}/{key}", get(state_proof))
        .route("/txs", post(submit))
        .route("/txs/{id}", get(transaction))
        .route("/validators", get(validators))
        .route("/validators/{voter}", get(validator))
        .route("/signers", get(signers))
        .route("/evidence", get(evidence))
        .route("/forks", get(forks))
        .route("/events", get(events))
        .route("/tokens", get(tokens))
        .route("/collections", get(collections))

        .route("/node", get(node))
        .route("/mempool", get(mempool))
        .route("/offer", post(offer))
        .route("/call", post(call));
    #[cfg(feature = "governance")]
    let router = router.route("/governance", get(governance));
    #[cfg(feature = "ethereum")]
    let router = router.route("/rpc", post(ethereum_rpc));
    #[cfg(feature = "faucet")]
    let router = router.route("/faucet", get(faucet::about).post(faucet::give));
    #[cfg(feature = "names")]
    let router = router.route("/names", get(names)).route("/names/{name}", get(name)).route("/accounts/{address}/names", get(account_names));
    #[cfg(feature = "stream")]
    let router = router.route("/stream", get(stream));
    let router = router.fallback(apps).layer(DefaultBodyLimit::max(MAX_BODY)).layer(middleware::from_fn_with_state(api.clone(), per_client));

    match cors {
        true => router.layer(middleware::from_fn_with_state(api.clone(), allow_origins)).with_state(api),
        false => router.with_state(api),
    }
}

/// Lets pages from the allowed origins read the API's answers, and answers their browsers'
/// preflight questions.
async fn allow_origins(Live(api): Live<Api>, request: Request, next: Next) -> Response {
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

/// Serves the web apps' files for anything that is not a route: the wallet's under /wallet/, and
/// the explorer's for the rest, / being its index.html.
async fn apps(Live(api): Live<Api>, uri: Uri) -> Result<Response, ApiError> {
    let path = uri.path();

    if let Some(root) = &api.wallet {
        // Its files load relative to the page, so the page must end in a slash.
        if path == "/wallet" {
            return Ok((StatusCode::PERMANENT_REDIRECT, [(header::LOCATION, "/wallet/")]).into_response());
        }

        if let Some(wanted) = path.strip_prefix("/wallet/") {
            return app_file(root, wanted);
        }
    }

    match &api.explorer {
        Some(root) => app_file(root, path.trim_start_matches('/')),
        None => Err(ApiError::NotFound("no such route".to_string())),
    }
}

/// A web app's file, by its path below the app's directory.
fn app_file(root: &std::path::Path, wanted: &str) -> Result<Response, ApiError> {
    let missing = || ApiError::NotFound("no such route".to_string());
    let wanted = match wanted {
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
    /// Asked again too soon.
    #[cfg_attr(not(feature = "faucet"), allow(dead_code))]
    TooMany(String),
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
            ApiError::TooMany(message) => (StatusCode::TOO_MANY_REQUESTS, message),
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

    // Where vote rewards come from, and what the pool holds when they come from it.
    let vote_rewards = match api.vote_rewards {
        VoteRewardSource::Mint => json!({ "source": "mint" }),
        VoteRewardSource::Fees => {
            let pool = crate::registry::Accounts::new(&State::new(db.clone())).get(&REWARD_POOL)?.balance;

            json!({ "source": "fees", "pool": REWARD_POOL.to_string(), "balance": pool.to_string() })
        }
    };

    Ok(Json(json!({
        "genesis": genesis.id().to_string(),
        "chain_id": crate::crypto::ethereum::chain_id(&genesis.id()),
        "tip": tip.id().to_string(),
        "height": tip.header.height,
        "timestamp": tip.header.timestamp,
        "difficulty": tip.header.difficulty.to_string(),
        "work": work.to_string(),
        "hash": api.hash.name(),
        "coin": crate::cli::genesis::coin_json(&api.coin),
        "token_functions": &*api.token_functions,
        "federation": federation_json(&api, &State::new(db.clone()))?,
        "directory": crate::registry::changes::Changes::new(&State::new(db.clone())).directory()?.as_ref().map(crate::cli::genesis::directory_json),
        "collections": api.rules.standards_in(&State::new(db.clone()))?.collections.as_ref().map(crate::cli::genesis::collections_json),
        "finality": finality,
        "fees": fees,
        "vote_rewards": vote_rewards,
        "max_block_bytes": api.max_block_bytes,
        "bootnodes": api.bootnodes(&State::new(db.clone()))?,
        "faucet": faucet_json(&api),
        "swap": swap_json(&api),
        "dex": dex_json(&api),
    })))
}

/// The DEXes clients use: { contract, router }, each null when the node names none; null for a
/// node that names neither.
fn dex_json(api: &Api) -> Value {
    #[cfg(feature = "dex")]
    if api.dex_contract.is_some() || api.dex_router.is_some() {
        return json!({
            "contract": api.dex_contract.map(|contract| contract.to_string()),
            "router": api.dex_router.map(|router| to_hex(&router)),
        });
    }

    let _ = api;

    Value::Null
}

/// The swap contract clients use: { contract }, or null on a node that names none.
fn swap_json(api: &Api) -> Value {
    #[cfg(feature = "swap")]
    if let Some(contract) = api.swap_contract {
        return json!({ "contract": contract.to_string() });
    }

    let _ = api;

    Value::Null
}

/// The faucet's settings, for clients to show it by; null on a node that runs none.
fn faucet_json(api: &Api) -> Value {
    #[cfg(feature = "faucet")]
    if let Some(faucet) = &api.faucet {
        return faucet.json();
    }

    let _ = api;

    Value::Null
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
    let Some(rules) = api.rules.signers else {
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

#[cfg(feature = "governance")]
/// Governance: the consensus the tip's epoch runs and the one decided last, and the approvals
/// that still count in the tip's epoch, each change with what its approvals weigh and need.
async fn governance(Shared(api): Shared<Api>) -> Answer {
    let Some(rules) = &api.rules.governance else {
        return Err(ApiError::NotFound("this chain has no governance".to_string()));
    };
    let db = api.db.clone();
    let height = tip(&*db)?.header.height;
    let epoch = rules.finality.epoch_of(height);
    let state = State::new(db);
    let changes = Changes::new(&state);
    let decided = changes.decided()?;
    let ballot = changes.ballot()?.counting(epoch, rules.approval_epochs);
    let needed = rules.governance.needed(&state, epoch)?;
    let mut proposals = Vec::new();

    for change in ballot.changes() {
        let approvals: Vec<Value> = ballot
            .approvals
            .iter()
            .filter(|approval| approval.change == change)
            .map(|approval| json!({
                "account": approval.account.to_string(),
                "weight": rules.governance.weight(&state, epoch, &approval.account).ok().flatten().map_or_else(|| "0".to_string(), |weight| weight.to_string()),
                "epoch": approval.epoch,
                "counts_until": approval.epoch + rules.approval_epochs - 1,
            }))
            .collect();
        let mut value = json::change(&change);
        value["weight"] = changes.weight_of(&ballot, &change, epoch, &*rules.governance)?.to_string().into();
        value["needed"] = needed.to_string().into();
        value["approvals"] = approvals.into();
        proposals.push(value);
    }

    Ok(Json(json!({
        "kind": rules.governance.name(),
        "epoch": epoch,
        "active": changes.consensus_for(epoch)?.as_ref().map(|consensus| crate::cli::genesis::consensus_json(consensus, api.light.pow_window)),
        "version": decided.version,
        "decided": crate::cli::genesis::consensus_json(&decided.consensus, api.light.pow_window),
        "approval_epochs": rules.approval_epochs,
        "needed": needed.to_string(),
        "bootnodes": api.bootnodes(&state)?,
        "proposals": proposals,
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
        summaries.push(match stored_block_at(&*db, height)? {
            Some(block) => json::summary(&block, &api.reward),
            // Before the snapshot this node synced from, it keeps headers alone.
            None => json::pruned(&stored_header_at(&*db, height)?.ok_or_else(|| ApiError::Db(DbError::Corrupt(format!("no block at height {height}"))))?, &api.reward),
        });
    }

    Ok(Json(json!({
        "blocks": summaries,
        "next": (first > 0).then_some(first),
    })))
}

async fn block(Shared(api): Shared<Api>, Path(block): Path<String>) -> Answer {
    let (db, hash) = (api.db, api.hash);
    let (found, header) = if block == "latest" {
        (Some(tip(&*db)?), None)
    } else if block.starts_with("0x") {
        let id = block_id(&block, hash)?;

        (stored_block(&*db, &id)?, stored_header(&*db, &id)?)
    } else {
        let height = block
            .parse()
            .map_err(|_| ApiError::BadRequest(format!("not a height or a block id: {block}")))?;

        (stored_block_at(&*db, height)?, stored_header_at(&*db, height)?)
    };
    let found = match (found, header) {
        (Some(found), _) => found,
        // Before the snapshot this node synced from, it keeps headers alone.
        (None, Some(sealed)) => return Ok(Json(json::pruned(&sealed, &api.reward))),
        (None, None) => return Err(ApiError::NotFound(format!("no block {block}"))),
    };

    let id = found.id();
    let mut receipts = Vec::new();

    for tx in &found.txs {
        // A side-branch block's transactions may sit elsewhere on the main chain, or nowhere.
        receipts.push(stored_receipt(&*db, &tx.id(hash))?.filter(|receipt| receipt.block == id));
    }

    let mut value = json::block(&found, &receipts, hash, &api.reward);
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
        "blocks": side.iter().map(|block| json::summary(block, &api.reward)).collect::<Vec<_>>(),
        "tips": tips,
    })))
}

/// Indexed events, newest first: a contract's, a contract's of one name, or those that mention an
/// address, a page at a time.
async fn events(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let Some(index) = stored_index(&*db, IndexKind::Events)? else {
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
    let state = State::new(db.clone());
    let account = Accounts::new(&state).get(&address)?;
    #[cfg_attr(not(feature = "names"), allow(unused_mut))]
    let mut answer = json::account(&address, &account);

    // Its primary name, for a reverse lookup, while that name still resolves to it.
    #[cfg(feature = "names")]
    if let Some(names) = &api.rules.names {
        use crate::registry::names::Names;

        let now = tip(&*db)?.header.timestamp;
        answer["name"] = Names::new(&state).primary(&address, now, names.grace_ms)?.map(|name| names.display(&name)).into();
    }

    Ok(Json(answer))
}

/// Ethereum's JSON-RPC: answers come back with status 200 whatever they hold, as JSON-RPC's do.
#[cfg(feature = "ethereum")]
async fn ethereum_rpc(Shared(api): Shared<Api>, body: Bytes) -> Json<Value> {
    Json(rpc::rpc(Arc::new(api), &body).await)
}

/// The chain's names, or a 404 on a chain without them.
#[cfg(feature = "names")]
fn names_config(api: &Api) -> Result<&crate::registry::names::NamesConfig, ApiError> {
    api.rules.names.as_deref().ok_or_else(|| ApiError::NotFound("this chain has no names".to_string()))
}

/// The prices names are registered and renewed at now: governance's last decided, else genesis's.
#[cfg(feature = "names")]
fn name_prices(state: &State, names: &crate::registry::names::NamesConfig) -> Result<Vec<u256>, ApiError> {
    Ok(Changes::new(state).name_prices()?.unwrap_or_else(|| names.prices.clone()))
}

/// The chain's names: their suffix, what a year costs by length now and at genesis, the grace
/// period and the most years ahead a name is held.
#[cfg(feature = "names")]
async fn names(Shared(api): Shared<Api>) -> Answer {
    let names = names_config(&api)?;
    let state = State::new(api.db.clone());
    let mut answer = crate::cli::genesis::names_json(names);
    answer["genesis_prices"] = answer["prices"].clone();
    answer["prices"] = name_prices(&state, names)?.iter().map(u256::to_string).collect::<Vec<_>>().into();

    Ok(Json(answer))
}

/// A name, with or without the suffix, at the tip: whether it is active, in its grace period or
/// available, who holds it, what it resolves to, its text records, and for a name, what a year
/// of it costs.
#[cfg(feature = "names")]
async fn name(Shared(api): Shared<Api>, Path(name): Path<String>) -> Answer {
    use crate::registry::names::{NameStatus, Names, name_problem, price};

    let names = names_config(&api)?;
    let name = names.strip(&name);

    if let Some(problem) = name_problem(&name) {
        return Err(ApiError::BadRequest(format!("{name} is not a name: {problem}")));
    }

    let db = api.db.clone();
    let now = tip(&*db)?.header.timestamp;
    let state = State::new(db);
    let found = Names::new(&state).lookup(&name, now, names.grace_ms)?;
    let top_level = !name.contains('.');
    let per_year = match top_level {
        true => price(&name_prices(&state, names)?, &name, 1).map(|price| price.to_string()),
        false => None,
    };
    let record = found.as_ref().map(|found| &found.record);

    Ok(Json(json!({
        "name": name,
        "display": names.display(&name),
        "status": found.as_ref().map_or(NameStatus::Available, |found| found.status).name(),
        "owner": record.map(|record| record.owner.to_string()),
        "target": record.and_then(|record| record.target).map(|target| target.to_string()),
        "resolves_to": found.as_ref().filter(|found| found.status == NameStatus::Active).and_then(|found| found.record.target).map(|target| target.to_string()),
        "expires": found.as_ref().map(|found| found.expires),
        "grace_ends": found.as_ref().map(|found| found.expires.saturating_add(names.grace_ms)),
        "texts": record.map_or_else(serde_json::Map::new, |record| record.texts.iter().map(|text| (text.key.clone(), Value::from(text.value.clone()))).collect()),
        "price_per_year": per_year,
    })))
}

/// The names an account owns that still stand, in name order, a page at a time: each with its
/// status and when it expires.
#[cfg(feature = "names")]
async fn account_names(Shared(api): Shared<Api>, Path(address): Path<String>, Query(query): Query<HashMap<String, String>>) -> Answer {
    use crate::registry::names::{Names, stored_owned_names};

    let names = names_config(&api)?;
    let address = parse_address(&address)?;
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_ACCOUNT_TXS).clamp(1, MAX_ACCOUNT_TXS) as usize;
    let start = query.get("cursor").map_or("", String::as_str);
    let db = api.db.clone();
    let now = tip(&*db)?.header.timestamp;
    let (owned, next) = stored_owned_names(&*db, &address, start, limit)?;
    let state = State::new(db);
    let registry = Names::new(&state);
    let mut listed = Vec::new();

    for name in owned {
        // Listed under it, but expired past its grace period, or a subname of an earlier
        // registration: no longer its.
        if let Some(found) = registry.lookup(&name, now, names.grace_ms)?
            && found.record.owner == address
        {
            listed.push(json!({
                "name": name,
                "display": names.display(&name),
                "status": found.status.name(),
                "target": found.record.target.map(|target| target.to_string()),
                "expires": found.expires,
            }));
        }
    }

    Ok(Json(json!({ "names": listed, "next": next })))
}

/// The transactions touching an address, newest first, a page at a time: from the address index,
/// for a node that keeps one. Each with what it did, as a block lists it.
async fn account_txs(Shared(api): Shared<Api>, Path(address): Path<String>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let Some(index) = stored_index(&*db, IndexKind::Addresses)? else {
        return Err(ApiError::NotFound("this node keeps no address index; start it with --index-addresses".to_string()));
    };
    let address = parse_address(&address)?;
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_ACCOUNT_TXS).clamp(1, MAX_ACCOUNT_TXS) as usize;
    let cursor = query
        .get("cursor")
        .map(|cursor| from_hex(cursor).ok_or_else(|| ApiError::BadRequest("cursor is not hex".to_string())))
        .transpose()?;
    let (found, next) = stored_address_txs(&*db, &address, cursor.as_deref(), limit)?;
    let mut txs = Vec::with_capacity(found.len());

    for indexed in &found {
        let block = stored_block(&*db, &indexed.block)?;
        let receipt = stored_receipt(&*db, &indexed.tx)?;

        // A reorg between the reads took it off the main chain: the next page read has it right.
        if let (Some(block), Some(receipt)) = (block, receipt)
            && receipt.block == indexed.block
            && let Some(signed) = block.txs.get(indexed.index as usize)
        {
            txs.push(json::indexed_tx(indexed, signed, &receipt, api.hash));
        }
    }

    let tip = tip(&*db)?.header.height;

    Ok(Json(json!({
        "txs": txs,
        "next": next.map(|next| to_hex(&next)),
        "indexed_from": index.from,
        "catching_up_to": (!index.caught_up(tip)).then(|| index.keep.lowest(tip)),
    })))
}

async fn contract(Shared(api): Shared<Api>, Path(address): Path<String>) -> Answer {
    let db = api.db;
    let address = parse_address(&address)?;
    let state = Arc::new(State::new(db));
    let record = Contracts::new(&state)
        .get(&address)?
        .ok_or_else(|| ApiError::NotFound(format!("no contract at {address}")))?;
    let token = Tokens::new(&state).get(&address)?;
    let collection = Collections::new(&state).get(&address)?;

    Ok(Json(json::contract(&address, &record, token.as_ref(), collection.as_ref())))
}

/// What a light client starts from: the chain's light parameters and its genesis header, encoded,
/// for it to check against the genesis id it trusts.
async fn light(Shared(api): Shared<Api>) -> Answer {
    let genesis = stored_header_at(&*api.db, 0)?.ok_or_else(|| ApiError::NotFound("the chain has no genesis yet".to_string()))?;

    Ok(Json(json!({
        "genesis": genesis.id().to_string(),
        "params": to_hex(&wincode::serialize(&*api.light).expect("parameters always encode")),
        "header": to_hex(&genesis.hashable_bytes_with_tag()),
    })))
}

/// Main-chain headers with their seals, encoded, from ?from= on, at most ?count=.
async fn headers(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let from = number(&query, "from")?.unwrap_or(1);
    let count = number(&query, "count")?.unwrap_or(u64::from(MAX_LIGHT_HEADERS)).min(u64::from(MAX_LIGHT_HEADERS));
    let mut headers = Vec::new();

    for height in from..from.saturating_add(count) {
        match stored_header_at(&*api.db, height)? {
            Some(sealed) => headers.push(to_hex(&sealed.hashable_bytes_with_tag())),
            None => break,
        }
    }

    Ok(Json(json!({ "headers": headers })))
}

/// The light update a block keeps, encoded: proofs of the voters genesis or a checkpoint fixed.
async fn light_update(Shared(api): Shared<Api>, Path(block): Path<String>) -> Answer {
    let block = block_id(&block, api.hash)?;
    let update = stored_light_update(&*api.db, &block)?
        .ok_or_else(|| ApiError::NotFound(format!("block {block} keeps no light update")))?;

    Ok(Json(json!({ "update": to_hex(&update.hashable_bytes_with_tag()) })))
}

/// Finality for light clients, an epoch at a time from ?from= on, at most ?count=: each main-chain
/// checkpoint's justification, none for genesis, and the light update its block keeps, if any,
/// encoded. It stops before the first checkpoint past genesis it holds no justification for.
async fn light_finality(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let interval = api
        .light
        .finality
        .and_then(|finality| finality.checkpoint_interval())
        .ok_or_else(|| ApiError::NotFound("this chain's finality does not vote".to_string()))?;
    let from = number(&query, "from")?.unwrap_or(0);
    let count = number(&query, "count")?.unwrap_or(MAX_FINALITY_STEPS).min(MAX_FINALITY_STEPS);
    let top = tip(&*api.db)?.header.height;
    let steps = stored_light_finality(&*api.db, interval, from, count, top)?;

    Ok(Json(json!({ "steps": steps.iter().map(|step| to_hex(&step.hashable_bytes_with_tag())).collect::<Vec<_>>() })))
}

/// What a light client starts from at this node's finalized checkpoint, for one that takes the
/// node's on first use. 404 before anything past genesis is final.
async fn light_checkpoint_final(Shared(api): Shared<Api>) -> Answer {
    let (_, finalized) = stored_finality(&*api.db)?;
    let finalized = finalized.ok_or_else(|| ApiError::NotFound("nothing past genesis is final yet".to_string()))?;

    light_checkpoint_at(&api, finalized.block)
}

/// What a light client starts from at the checkpoint named, rather than at genesis.
async fn light_checkpoint(Shared(api): Shared<Api>, Path(block): Path<String>) -> Answer {
    let block = block_id(&block, api.hash)?;

    light_checkpoint_at(&api, block)
}

/// The checkpoint, and from its epoch's start back, the headers and light updates that let a
/// light client start there, encoded: a CheckpointStart. 404 for a block not on the main chain,
/// 400 for one that opens no epoch under voting finality.
fn light_checkpoint_at(api: &Api, block: Hash) -> Answer {
    let (epoch, start) = match stored_checkpoint_start(&*api.db, &api.light, &block)? {
        Ok(found) => found,
        Err(problem @ (StartProblem::NotOnChain | StartProblem::Missing(_))) => return Err(ApiError::NotFound(format!("block {block}: {problem}"))),
        Err(problem) => return Err(ApiError::BadRequest(format!("block {block}: {problem}"))),
    };
    let height = start.headers.last().map_or(0, |sealed| sealed.header.height);

    Ok(Json(json!({
        "checkpoint": { "epoch": epoch, "height": height, "block": block.to_string() },
        "start": to_hex(&start.hashable_bytes_with_tag()),
    })))
}

/// The justifications this node holds for main-chain checkpoints from ?from= on, encoded, at most
/// ?count=, under the kinds of finality that vote.
async fn justifications(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let interval = api
        .light
        .finality
        .and_then(|finality| finality.checkpoint_interval())
        .ok_or_else(|| ApiError::NotFound("this chain's finality does not vote".to_string()))?;
    let from = number(&query, "from")?.unwrap_or(1).max(1);
    let count = number(&query, "count")?.unwrap_or(MAX_PROOF_JUSTIFICATIONS).min(MAX_PROOF_JUSTIFICATIONS);
    let top = tip(&*api.db)?.header.height;
    let mut found = Vec::new();

    for epoch in from..from.saturating_add(count) {
        let Some(height) = epoch.checked_mul(interval).filter(|height| *height <= top) else {
            break;
        };
        let Some(sealed) = stored_header_at(&*api.db, height)? else {
            break;
        };

        match stored_justification(&*api.db, &sealed.id())? {
            Some(justification) => found.push(to_hex(&justification.hashable_bytes_with_tag())),
            None => break,
        }
    }

    Ok(Json(json!({ "justifications": found })))
}

/// The most justifications one request answers.
const MAX_PROOF_JUSTIFICATIONS: u64 = 64;

/// A proof of one chain-state entry, or of its absence, against the state root of the tip, or of
/// the recent main-chain block ?block= names.
async fn state_proof(
    Shared(api): Shared<Api>,
    Path((column, key)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Answer {
    let named = column;
    let column = STATE_COLUMNS.into_iter().find(|column| column.name() == named).ok_or_else(|| {
        let names: Vec<&str> = STATE_COLUMNS.iter().map(|column| column.name()).collect();

        ApiError::BadRequest(format!("{named} is not a chain-state column; they are {}", names.join(", ")))
    })?;
    let key = from_hex(&key).ok_or_else(|| ApiError::BadRequest(format!("the key is not hex: {key}")))?;

    let at = query.get("block").map(|block| block_id(block, api.hash)).transpose()?;
    // One snapshot: the tip, its tree and the value are all from one commit.
    let (state, sealed) = match at {
        None => {
            let tip = tip(&*api.db)?;

            (Arc::new(State::new(api.db.clone())), tip.sealed_header())
        }
        Some(block) => {
            let state = stored_state_at(api.db.clone(), &block)?.ok_or_else(|| {
                ApiError::NotFound(format!("block {block} is not on the main chain, or older than this node can prove at"))
            })?;
            let sealed = stored_header(&*api.db, &block)?.ok_or_else(|| ApiError::NotFound(format!("no block {block}")))?;

            (state, sealed)
        }
    };
    let value = state.get(column, &key)?;
    let proof = StateTree::new(&state, api.hash).prove(column, &key)?;

    Ok(Json(json::state_proof(&sealed, column, &key, value.as_deref(), &proof)))
}

/// A proof that a transaction's receipt is in its block's receipts root.
async fn receipt_proof(Shared(api): Shared<Api>, Path(id): Path<String>) -> Answer {
    let db = api.db.clone();
    let id = block_id(&id, api.hash)?;
    // One snapshot: the receipt and its block's others are from one commit, so a reorg never
    // lands between them.
    let receipt = stored_receipt(&*db, &id)?.ok_or_else(|| ApiError::NotFound(format!("transaction {id} is not on the main chain, or not yet")))?;
    let block = stored_block(&*db, &receipt.block)?.ok_or_else(|| ApiError::Db(DbError::Corrupt(format!("block {} is not stored", receipt.block))))?;
    let mut committed: Vec<CommittedReceipt> = Vec::with_capacity(block.txs.len());

    for tx in &block.txs {
        let tx = tx.id(api.hash);
        let stored = stored_receipt(&*db, &tx)?
            .filter(|stored| stored.block == receipt.block)
            .ok_or_else(|| ApiError::Db(DbError::Corrupt(format!("transaction {tx} of block {} has no receipt there", receipt.block))))?;
        committed.push(stored.committed(tx));
    }

    let leaves: Vec<Hash> = committed.iter().map(|receipt| receipt.leaf(api.hash)).collect();
    let index = receipt.index as usize;

    if index >= leaves.len() {
        return Err(ApiError::Db(DbError::Corrupt(format!("transaction {id} is past the end of its block"))));
    }

    let path = merkle::audit_path(api.hash, &leaves, index);
    // The transaction too, against tx_root: who sent what, which the receipt does not say.
    let txs: Vec<Hash> = block.txs.iter().map(|tx| tx.leaf(api.hash)).collect();
    let tx_path = merkle::audit_path(api.hash, &txs, index);

    Ok(Json(json::receipt_proof(&block, &committed[index], index, leaves.len(), &path, &tx_path)))
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

/// The bridges' federation in force, with its version: governance's last, else genesis's as 0.
fn federation_json(api: &Api, state: &State) -> Result<Value, ApiError> {
    let decided = crate::registry::changes::Changes::new(state).federation()?;
    let in_force = decided.map(|decided| (decided.version, decided.federation)).or_else(|| api.federation.as_deref().map(|federation| (0, federation.clone())));

    Ok(match in_force {
        Some((version, federation)) => {
            let mut value = crate::cli::genesis::federation_json(&federation);
            value["version"] = version.into();

            value
        }
        None => Value::Null,
    })
}

/// The collection registry by symbol, a page at a time, as /tokens lists tokens.
async fn collections(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let limit = number(&query, "limit")?.unwrap_or(DEFAULT_TOKENS).clamp(1, MAX_TOKENS) as usize;
    let cursor = query
        .get("cursor")
        .map(|cursor| from_hex(cursor).ok_or_else(|| ApiError::BadRequest("cursor is not hex".to_string())))
        .transpose()?;
    let (found, next) = stored_collections(&*api.db, query.get("symbol").map(String::as_str), cursor.as_deref(), limit)?;

    Ok(Json(json!({
        "collections": found.iter().map(json::collection).collect::<Vec<_>>(),
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
            MempoolError::Full | MempoolError::Syncing => ApiError::Unavailable(err.to_string()),
            MempoolError::Db(err) => ApiError::Db(err),
        }),
        Err(err @ (SubmitError::Stopped | SubmitError::Busy)) => Err(ApiError::Unavailable(err.to_string())),
    }
}

/// The fee offer sure to cover a transaction sent now, keeping what the body sets itself:
/// gas_limit, max_price and tip. The body's tx is the transaction signed with any offer, which
/// weighs what the real one will. Nothing, on a chain without fees.
async fn offer(Shared(api): Shared<Api>, body: Bytes) -> Answer {
    let body = json_body(&body)?;
    let probe = signed_tx(&body)?;
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
    let (max_price, tip_chosen) = (amount("max_price")?, amount("tip")?);
    let header = tip(&*api.db)?.header;
    let base = api.fees.as_ref().map_or(u256::ZERO, |fees| fees.pricing().next_price(header.base_price, header.gas_used, fees.block_gas_limit()));
    let open = api.fees.as_ref().map_or(0, |fees| fees.offer(base, api.tx_fuel, &probe, OfferChoice::default()).gas_limit);
    // What runs code, or names, is tried first: what it would use, and whether it would fail.
    let ran = match probe.tx {
        Tx::ContractCall { .. } | Tx::EvmCall { .. } | Tx::Ethereum { .. } | Tx::Deploy { .. } | Tx::Upgrade { .. } | Tx::Name { .. } | Tx::PrimaryName { .. } => Some(dry_run_off_loop(&api, probe.clone(), open, base).await?),
        _ => None,
    };
    let offer = match &api.fees {
        None => FeeOffer::NONE,
        Some(fees) => {
            // A run that fails says nothing of what a later one needs: that gets the fuel cap.
            let mut estimated = ran.as_ref().filter(|ran| ran.failure.is_none()).map(|ran| fees.estimated(api.tx_fuel, &probe, ran.gas_used));

            // What a run used, and a quarter, may still be too little: a call inside another
            // gets 63/64 of what is left, so nested calls need more than they use. Tried at the
            // limit, and raised until a run succeeds at it, at most to the open one it did.
            if gas_limit.is_none() {
                while let Some(limit) = estimated.filter(|limit| *limit < open) {
                    if dry_run_off_loop(&api, probe.clone(), limit, base).await?.failure.is_none() {
                        break;
                    }

                    estimated = Some(limit.saturating_add(limit / 4).max(limit + 1).min(open));
                }
            }

            let chosen = OfferChoice { gas_limit: gas_limit.or(estimated), max_price, tip: tip_chosen };

            fees.offer(base, api.tx_fuel, &probe, chosen)
        }
    };
    let mut answer = json::fee_offer(&offer);
    answer["estimate"] = ran.as_ref().map_or(Value::Null, json::dry_run);

    Ok(Json(answer))
}

/// Runs a dry run on a blocking thread, since the API answers everything on one.
async fn dry_run_off_loop(api: &Api, probe: SignedTx, gas_limit: u64, base: u256) -> Result<DryRun, ApiError> {
    let (db, rules, vm) = (api.db.clone(), api.rules.clone(), api.vm.clone());

    tokio::task::spawn_blocking(move || dry_run(db, &rules, &vm, &probe, gas_limit, base))
        .await
        .map_err(|err| ApiError::Unavailable(format!("the dry run stopped: {err}")))?
        .map_err(ApiError::Db)
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
    let (db, vm, limits, function) = (api.db.clone(), api.vm.clone(), api.limits, Hash::Xxh3_64(selector(function)));
    let header = tip(&*db)?.header;
    let block = BlockAt {
        height: header.height,
        time: header.timestamp,
        producer: header.producer,
        fuel_per_gas: api.fees.as_ref().map_or(1, |fees| fees.schedule().fuel_for(1).max(1)),
    };
    // On a blocking thread, since the API answers everything on one.
    let outcome = tokio::task::spawn_blocking(move || {
        let state = Arc::new(State::new(db));
        let record = Contracts::new(&state)
            .get(&contract)?
            .ok_or_else(|| ApiError::NotFound(format!("no contract at {contract}")))?;
        let context = CallContext {
            sender,
            contract,
            value: u256::ZERO,
            block,
        };

        Ok::<_, ApiError>(vm.call(&state, &record, context, &function, &args, limits))
    })
    .await
    .map_err(|err| ApiError::Unavailable(format!("the call stopped: {err}")))??;

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
    Live(api): Live<Api>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    use tokio_stream::{StreamExt, wrappers::BroadcastStream, wrappers::errors::BroadcastStreamRecvError};

    let events = api
        .node
        .as_ref()
        .and_then(NodeHandle::subscribe)
        .ok_or_else(|| ApiError::NotFound("this node does not stream; start it with --stream".to_string()))?;
    let (hash, reward) = (api.hash, api.reward);
    let events = BroadcastStream::new(events).map(move |event| {
        let (name, data) = match event {
            Ok(event) => json::node_event(&event, hash, &reward),
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
