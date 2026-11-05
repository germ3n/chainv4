//! An HTTP API over the committed chain, answering in JSON. It runs on a thread of its own and
//! reads the database directly, so a slow client never holds up the node loop. The one write,
//! POST /txs, hands a signed transaction to the node through a NodeHandle.

use {
    crate::chain::fees::OfferChoice,
    crate::chain::{
        DryRun, RangeLimits, RangeProblem, Rules, dry_run, stored_block_range, Coin, EventFilter, FinalityConfig, Fees, Genesis, IndexKind, REWARD_POOL, VoteRewardSource, scan_address_txs, address_txs_from, stored_index, stored_member_index, stored_multisigs_of, stored_events, stored_block, stored_block_at, stored_evidence, stored_finality, stored_on_main_chain,
        stored_receipt, stored_tip, stored_tips, stored_work, CommittedReceipt, stored_header, stored_header_at, stored_justification,
        stored_light_finality, stored_light_update, stored_state_at, stored_checkpoint_start, stored_split, StartProblem,
    },
    crate::crypto::hashable::Hashable,
    crate::light::{LightParams, MAX_FINALITY_STEPS, MAX_LIGHT_HEADERS},
    crate::crypto::merkle,
    crate::crypto::keypair::PublicKey,
    crate::registry::signers::{Approval, SignerSet, Signers},
    crate::registry::changes::Changes,
    crate::registry::validators::Validators,
    crate::structs::vote::Checkpoint,
    crate::db::{Column, Committed, Database, DbError, Read, STATE_COLUMNS, State, StateTree},
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
    axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, Path, Query, Request, State as Live},
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
#[cfg(feature = "governance")]
use crate::chain::stored_decisions;

mod proxies;
#[cfg(feature = "faucet")]
mod faucet;
mod filters;
mod json;
mod mining;
pub mod sharing;
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
#[document(
    summary = "The least the largest request body may be: 2 MiB, room for the largest deploy, hex-encoded, as a module or source is at most half the least block. On a chain whose limits name a max_tx_bytes, the cap is room for one of those hex-encoded instead, whenever that is more, following governance's limits as they stand.",
    category = "Node",
    see_also = "body_cap",
)]
pub const MAX_BODY: usize = 2 * 1024 * 1024;
/// What a body holds besides a transaction's hex: its JSON, and a few fields beside it.
const BODY_SLACK: usize = 64 * 1024;

#[document(
    summary = "The largest request body the API takes for a chain's most bytes a transaction may take: room for one hex-encoded in its JSON, and never less than MAX_BODY. Past it, 413.",
    category = "Node",
    params = { max_tx_bytes = "The chain's max_tx_bytes as it stands, or None when its limits name none." },
    see_also = "MAX_BODY",
)]
pub fn body_cap(max_tx_bytes: Option<u64>) -> usize {
    match max_tx_bytes {
        Some(bytes) => usize::try_from(bytes).unwrap_or(usize::MAX).saturating_mul(2).saturating_add(BODY_SLACK).max(MAX_BODY),
        None => MAX_BODY,
    }
}

#[document(
    summary = "How many block ranges one client may have under way at once unless --tune http.range-per-ip says otherwise: a scan asks for one at a time.",
    category = "Node",
    see_also = "ApiLimits::range_per_ip",
)]
pub const RANGE_PER_IP: usize = 2;
#[document(
    summary = "How long a block range waits its turn before the node says it is too busy, unless --tune http.range-wait says otherwise.",
    category = "Node",
    see_also = "ApiLimits::range_wait",
)]
pub const RANGE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

#[document(
    summary = "How many requests one client may have in flight at once unless --http-per-ip says otherwise: past it, 429. A client is its IP, or behind a proxy on the node's own network, the one the proxy says it forwards for.",
    category = "Node",
    see_also = "ApiOptions::per_ip",
)]
pub const DEFAULT_PER_IP: usize = 64;

#[document(
    summary = "How many mining WebSocket relays one client may hold open at once unless --tune http.relays-per-ip says otherwise. Each opens a connection into the mining port on this machine, which sees them all as loopback, so they are capped here by who asked rather than there. Past it, 503, as the port itself answers when full.",
    category = "Node",
    see_also = "ApiLimits::relays_per_ip",
)]
pub const MINING_RELAYS_PER_IP: usize = 32;

#[document(
    summary = "What JSON-RPC at POST /rpc answers within, as geth's --rpc.* flags set: set with --tune's rpc. keys. Public providers refuse filters past such limits too.",
    category = "Node",
    see_also = "ApiLimits::rpc",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RpcLimits {
    #[document(summary = "The most blocks one eth_getLogs scans: 10,000 by default.")]
    pub log_blocks: u64,
    #[document(summary = "The most addresses an eth_getLogs filter may name: 1,000 by default.")]
    pub log_addresses: usize,
    #[document(summary = "The most topics an eth_getLogs filter may name at one place: 1,000 by default.")]
    pub log_topics: usize,
    #[document(summary = "The most requests one batch may hold: 100 by default. Each is answered in order, and one may run a contract under the whole fuel cap, so a body may not line up more than a client could send as separate requests.")]
    pub batch: usize,
    #[document(summary = "The most blocks of fee history one eth_feeHistory gives: 1,024 by default.")]
    pub fee_history: u64,
}

impl RpcLimits {
    #[document(parent = "RpcLimits", summary = "What every node answered within before these were tuned.")]
    pub const DEFAULT: RpcLimits = RpcLimits { log_blocks: 10_000, log_addresses: 1_000, log_topics: 1_000, batch: 100, fee_history: 1_024 };
}

impl Default for RpcLimits {
    fn default() -> RpcLimits {
        RpcLimits::DEFAULT
    }
}

#[document(
    summary = "What the API takes from each client and answers within, beside --http-per-ip: requests a second, the largest listing page, block ranges and mining relays under way at once, what a block range answers within, JSON-RPC's limits, how much a filtered listing scans, and what a suggested fee offers over the base price. None of it is consensus. Set with --http-rate, --api-max-page, --suggest-multiplier and --tune's http., range., rpc. and filter. keys.",
    category = "Node",
    see_also = "ApiOptions::limits",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiLimits {
    #[document(summary = "How many requests one client may make a second, --http-rate: a bucket that holds a second's worth and fills at that rate, so a page asking for several things at once goes through, and a client that keeps asking faster gets 429 with Retry-After. 0, the default, counts nothing.")]
    pub rate: u32,
    #[document(summary = "The most any listing returns at once, --api-max-page, in place of each listing's own maximum, its default never above it: None keeps each listing's own, 100 to 1,000.")]
    pub max_page: Option<u64>,
    #[document(summary = "How many block ranges one client may have under way at once: RANGE_PER_IP by default.")]
    pub range_per_ip: usize,
    #[document(summary = "How long a block range waits its turn before the node says it is too busy: RANGE_WAIT by default.")]
    pub range_wait: std::time::Duration,
    #[document(summary = "How many mining WebSocket relays one client may hold open at once: MINING_RELAYS_PER_IP by default.")]
    pub relays_per_ip: usize,
    #[document(summary = "What GET /blocks/range answers within: RangeLimits::SERVED by default.")]
    pub range: RangeLimits,
    #[document(summary = "What POST /rpc answers within.")]
    pub rpc: RpcLimits,
    #[document(summary = "The most index entries a filtered listing, GET /accounts/{address}/txs, looks at, matches or not, before it answers with fewer and a cursor: 1,000 by default.")]
    pub filter_scan: usize,
    #[document(summary = "How many times the next base price a suggested fee offers, --suggest-multiplier: what eth_gasPrice says, and the max price POST /offer and the faucet fill in when left open, under a base fee. 2 by default, so an offer outlasts a few blocks of the base price rising. A chain whose senders bid suggests the base price plus the tip whatever this is.")]
    pub suggest: u64,
}

impl ApiLimits {
    #[document(parent = "ApiLimits", summary = "What every node took and answered within before these were tuned.")]
    pub const DEFAULT: ApiLimits = ApiLimits {
        rate: 0,
        max_page: None,
        range_per_ip: RANGE_PER_IP,
        range_wait: RANGE_WAIT,
        relays_per_ip: MINING_RELAYS_PER_IP,
        range: RangeLimits::SERVED,
        rpc: RpcLimits::DEFAULT,
        filter_scan: filters::MAX_TXS_SCANNED,
        suggest: 2,
    };
}

impl Default for ApiLimits {
    fn default() -> ApiLimits {
        ApiLimits::DEFAULT
    }
}

/// How many a listing returns: what the client asked for, or the listing's default, at most its
/// own maximum or --api-max-page's in its place.
fn page(tuned: &ApiLimits, query: &HashMap<String, String>, default: u64, most: u64) -> Result<u64, ApiError> {
    let most = tuned.max_page.unwrap_or(most);

    Ok(number(query, "limit")?.unwrap_or(default.min(most)).min(most))
}

/// Requests each client may make a second, as a bucket per client that holds a second's worth and
/// fills at that rate. A bucket left alone for a second is full, the same as none, so those go
/// once there are many.
struct Rate {
    per_second: u32,
    buckets: std::sync::Mutex<HashMap<IpAddr, Bucket>>,
}

struct Bucket {
    tokens: f64,
    at: std::time::Instant,
}

/// How many clients' buckets are kept before the full ones are let go.
const MOST_BUCKETS: usize = 4_096;

impl Rate {
    fn new(per_second: u32) -> Rate {
        Rate { per_second, buckets: Default::default() }
    }

    /// Takes one request from the client's bucket: false when it is empty.
    fn take(&self, ip: IpAddr, now: std::time::Instant) -> bool {
        if self.per_second == 0 {
            return true;
        }

        let rate = f64::from(self.per_second);
        let mut buckets = self.buckets.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        if buckets.len() >= MOST_BUCKETS {
            buckets.retain(|_, bucket| now.saturating_duration_since(bucket.at) < std::time::Duration::from_secs(1));
        }

        let bucket = buckets.entry(ip).or_insert(Bucket { tokens: rate, at: now });
        bucket.tokens = (bucket.tokens + now.saturating_duration_since(bucket.at).as_secs_f64() * rate).min(rate);
        bucket.at = now;

        match bucket.tokens >= 1.0 {
            true => {
                bucket.tokens -= 1.0;
                true
            }
            false => false,
        }
    }
}

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
    reward: crate::chain::RewardSchedule,
    /// The fees last built from a decided config, kept between requests.
    built_fees: Arc<crate::chain::BuiltFees>,
    /// Where vote rewards come from, and so whether the pool is worth showing.
    vote_rewards: VoteRewardSource,
    /// Genesis's bootnodes, which stand until governance decides others.
    genesis_bootnodes: Arc<Vec<String>>,
    /// Genesis itself, for what governance may change to show genesis's while it has not.
    #[cfg(any(feature = "governance", feature = "anchoring"))]
    genesis: Arc<Genesis>,
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
    /// The releases the node fetches.
    #[cfg(feature = "updates")]
    releases: Option<Arc<crate::node::Releases>>,
    /// Requests in flight, by client.
    in_flight: Arc<InFlight>,
    /// Whose turn it is to have a block range built.
    ranges: RangeGate,
    /// Where workers reach the mining port from outside, as GET /mining says.
    mining_public: Option<Arc<String>>,
    mining_onion: Option<Arc<MiningOnion>>,
    /// What it shares of itself, and the token that shows everything.
    sharing: sharing::Sharing,
    report_token: Option<Arc<Vec<u8>>>,
    /// Besides loopback, those whose X-Forwarded-For the API believes: the reverse proxies in front
    /// of it. None named trusts loopback alone.
    trusted_proxies: proxies::TrustedProxies,
    /// Mining WebSocket relays open now, by client, so one cannot open a flood of them into the
    /// mining port, which sees them all as loopback.
    mining_relays: Arc<InFlight>,
    /// What it takes from each client and answers within, beside the requests in flight.
    tuned: ApiLimits,
    /// Everything its operator tuned, node and API alike, for GET /node's limits.
    tuning: Arc<crate::node::Tuning>,
    /// How many requests each client has made lately, for --http-rate.
    rate: Arc<Rate>,
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

/// Who asks: the connection's IP, or, when the connection is from a reverse proxy the node trusts,
/// the last address X-Forwarded-For names, which that proxy added itself. Loopback is always
/// trusted, as the proxy on the same host connects from it; any other private address is trusted
/// only when --trusted-proxy named it, so that a second container or box on a shared private
/// network cannot spoof a client's address and slip past the per-IP limits.
fn asker(peer: SocketAddr, headers: &HeaderMap, trusted_proxies: &proxies::TrustedProxies) -> IpAddr {
    let trusted = peer.ip().is_loopback() || trusted_proxies.contains(&peer.ip());
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit(',').next())
        .and_then(|last| last.trim().parse().ok());

    match (trusted, forwarded) {
        (true, Some(ip)) => ip,
        _ => peer.ip(),
    }
}

/// Turns away a client asking faster than --http-rate, or with too many requests in flight
/// already, 429, so one cannot keep the rest waiting; and holds a body to what the chain's largest
/// transaction takes, 413 past it. A stream, open for as long as the client listens, is not
/// counted.
async fn per_client(Live(api): Live<Api>, axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>, request: Request, next: Next) -> Response {
    if request.uri().path() == "/stream" {
        return next.run(request).await;
    }

    let ip = asker(peer, request.headers(), &api.trusted_proxies);

    if !api.rate.take(ip, std::time::Instant::now()) {
        let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": format!("more than {} requests a second from you: slow down", api.rate.per_second) }))).into_response();
        response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));

        return response;
    }

    let cap = match request.method() == Method::POST {
        true => api.body_cap(),
        false => MAX_BODY,
    };
    let request = request.map(|body| axum::body::Body::new(http_body_util::Limited::new(body, cap)));

    match InFlight::enter(&api.in_flight, ip) {
        Some(_counted) => next.run(request).await,
        None => (StatusCode::TOO_MANY_REQUESTS, Json(json!({ "error": "too many requests at once from you: wait for some to answer" }))).into_response(),
    }
}

/// Who may have a block range built now. They work out proofs as they are asked, so a client has few
/// under way, and only so many are built at once by anyone, leaving the machine's other cores to the
/// node and the rest of the API, however many ask.
#[derive(Clone)]
struct RangeGate {
    asks: Arc<InFlight>,
    builders: Arc<tokio::sync::Semaphore>,
    wait: std::time::Duration,
}

/// A client's turn to have a range built: given back as it drops.
struct RangeTurn {
    _asking: Counted,
    _building: tokio::sync::OwnedSemaphorePermit,
}

impl RangeGate {
    fn new(builders: usize, wait: std::time::Duration, per_ip: usize) -> RangeGate {
        RangeGate {
            asks: Arc::new(InFlight { per_ip, counts: Default::default() }),
            builders: Arc::new(tokio::sync::Semaphore::new(builders)),
            wait,
        }
    }

    /// Waits for a turn, which costs nothing and goes in order, so a few clients at once simply take
    /// a little longer. A client with too many under way is turned away at once, and anyone is once
    /// the wait runs out.
    async fn enter(&self, ip: IpAddr) -> Result<RangeTurn, ApiError> {
        let Some(asking) = InFlight::enter(&self.asks, ip) else {
            return Err(ApiError::TooMany("too many block ranges at once from you: ask for one at a time".to_string()));
        };
        let Ok(Ok(building)) = tokio::time::timeout(self.wait, self.builders.clone().acquire_owned()).await else {
            return Err(ApiError::Unavailable("the node is busy working out proofs: try again shortly".to_string()));
        };

        Ok(RangeTurn { _asking: asking, _building: building })
    }
}

/// The API as a handler sees it: its database a snapshot taken as the request arrived, never
/// moving under it however many keys it reads. Middleware, which reads nothing, takes the API as
/// it is (Live).
struct Shared<T>(T);

impl FromRequestParts<Api> for Shared<Api> {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(_: &mut axum::http::request::Parts, api: &Api) -> Result<Shared<Api>, std::convert::Infallible> {
        let mut snapped = Api { db: api.live.snapshot(), ..api.clone() };

        // What governance decided stands in for genesis's, as of the snapshot. A database that
        // fails here fails the handler's own reads too, which say so.
        if let Err(err) = snapped.govern() {
            log::warn!("[http] reading what governance decided: {err}");
        }

        Ok(Shared(snapped))
    }
}

impl Api {
    /// Puts what governance decided in the database's state in place of genesis's fees, limits
    /// and reward: what the next block runs with, which every answer about fees and calls reads.
    fn govern(&mut self) -> Result<(), DbError> {
        if self.rules.governance.is_none() {
            return Ok(());
        }

        let state = State::new(self.db.clone());
        let changes = crate::registry::changes::Changes::new(&state);
        let (fees, limits, reward) = (changes.fees()?, changes.limits()?, changes.reward()?);
        // Finality's settings as they stand for the next block, which dry runs run as.
        let tuned = match (&self.rules.governance, stored_tip(&*self.db)?) {
            (Some(governance), Some(tip)) => changes.finality_at(governance.finality.epoch_of(tip.header.height + 1))?,
            _ => None,
        };

        let mut rules = (*self.rules).clone();

        // Everything else governance decided, as the next block runs by it.
        if let Some(tip) = stored_tip(&*self.db)? {
            crate::chain::govern_rules(&mut rules, &state, tip.header.height + 1)?;
        }

        if fees.is_none() && limits.is_none() && reward.is_none() && tuned.is_none() {
            self.rules = Arc::new(rules);

            return Ok(());
        }

        if let (Some(governance), Some(tip), Some(genesis)) = (&self.rules.governance, stored_tip(&*self.db)?, stored_header_at(&*self.db, 0)?) {
            let finality = &governance.finality;
            (rules.staking, rules.signers) = crate::chain::switch::rules_at(&state, finality, finality.epoch_of(tip.header.height + 1), genesis.id())?;
        }

        if let Some(config) = fees {
            let fees = crate::chain::built_fees(&self.built_fees, config);
            (self.fees, rules.fees) = (Some(fees.clone()), Some(fees));
        }

        if let Some(limits) = limits {
            (self.limits, self.tx_fuel, self.max_block_bytes) = (limits.tx_limits, limits.tx_limits.fuel, limits.max_block_bytes);
        }

        self.reward.decided = reward;
        self.rules = Arc::new(rules);

        Ok(())
    }

    /// The largest body a POST may carry: room for the chain's largest transaction, as its limits
    /// stand now, read from the live database so a change governance just decided applies at once.
    fn body_cap(&self) -> usize {
        let genesis = self.rules.vm.max_tx_bytes;

        if self.rules.governance.is_none() {
            return body_cap(genesis);
        }

        let state = State::new(self.live.snapshot());

        match Changes::new(&state).limits() {
            Ok(limits) => body_cap(limits.and_then(|limits| limits.max_tx_bytes).or(genesis)),
            Err(err) => {
                log::warn!("[http] reading the chain's limits for a body's size: {err}");

                body_cap(genesis)
            }
        }
    }

    /// What this asker is shared of the node, and whether it bore a token the node does not take.
    fn shared_with(&self, peer: SocketAddr, headers: &HeaderMap) -> (sharing::Sharing, bool) {
        sharing::shared_with(self.sharing, self.report_token.as_deref().map(Vec::as_slice), peer, headers)
    }

    /// The chain's bootnodes now: the ones governance decided last, or genesis's.
    fn bootnodes(&self, state: &State) -> Result<Vec<String>, ApiError> {
        Ok(Changes::new(state).bootnodes()?.unwrap_or_else(|| (*self.genesis_bootnodes).clone()))
    }
}

#[document(summary = "Where the node's mining port is reached through Tor.", category = "Node", see_also = "ApiOptions")]
#[derive(Clone, Debug)]
pub enum MiningOnion {
    #[document(summary = "At this host:port, as --mining-onion gives it.")]
    At(String),
    #[document(summary = "On this port of the onion tor writes into this file, as --onion-file reads it, for a node started before tor made it: --mining-onion-port.")]
    InFile {
        file: std::path::PathBuf,
        port: u16,
    },
}

impl MiningOnion {
    #[document(parent = "MiningOnion", summary = "The host:port as it stands: None while tor has not made the onion yet.")]
    pub fn address(&self) -> Option<String> {
        match self {
            MiningOnion::At(at) => Some(at.clone()),
            MiningOnion::InFile { file, port } => Some(format!("{}:{port}", crate::net::tor::onion_host(&crate::net::tor::read_onion(file)?)?)),
        }
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
    #[document(summary = "The atomic swap contract on this chain, contracts/lua/swap.lua as deployed here, which GET /status names for the wallet and the CLI when the directory names none, and shows under operator either way; None for none.")]
    #[cfg(feature = "swap")]
    pub swap_contract: Option<Address>,
    #[document(summary = "The DEX on this chain, contracts/lua/dex.lua as deployed here, which GET /status names for the wallet when the directory names none, and shows under operator either way; None for none.")]
    #[cfg(feature = "dex")]
    pub dex_contract: Option<Address>,
    #[document(summary = "The EVM DEX's router on this chain, by its 20 bytes, which GET /status names when the directory names none, and shows under operator either way; None for none.")]
    #[cfg(feature = "dex")]
    pub dex_router: Option<[u8; 20]>,
    #[document(summary = "The threads the API answers on: as many as the machine has cores when 0.")]
    pub threads: usize,
    #[document(summary = "How many requests one client may have in flight at once; DEFAULT_PER_IP when 0.")]
    pub per_ip: usize,
    #[document(summary = "The releases the node fetches, which GET /release serves; None for a node that fetches none.")]
    #[cfg(feature = "updates")]
    pub releases: Option<Arc<crate::node::Releases>>,
    #[document(summary = "Where workers reach the node's mining port from outside, --mining-public, as GET /mining tells the apps; None for them to take the API's host.")]
    pub mining_public: Option<String>,
    #[document(summary = "Where workers reach the node's mining port through Tor, as GET /mining tells the apps that reach it at its onion; None for no such port.")]
    pub mining_onion: Option<MiningOnion>,
    #[document(summary = "What it shares of itself with whoever asks: by default, all but the addresses of its peers, its workers and itself, and its workers' names.")]
    pub sharing: sharing::Sharing,
    #[document(summary = "The token that shows everything to whoever bears it, --report-token; None for nobody off this machine to see it.")]
    pub report_token: Option<Vec<u8>>,
    #[document(summary = "Besides loopback, the reverse proxies whose X-Forwarded-For the API believes, --trusted-proxy, each an address or a host's name looked up again every few seconds. Empty trusts loopback alone, so no other host on a shared private network can spoof a client's address past the per-IP limits.")]
    pub trusted_proxies: Vec<String>,
    #[document(summary = "What it takes from each client and answers within, beside per_ip: ApiLimits::DEFAULT unless set.")]
    pub limits: ApiLimits,
    #[document(summary = "How big the engines it runs read-only calls on are: the node's own --lua-states and --wasm-cache-mb, so the API's take what the node's do.")]
    pub pools: crate::vm::EnginePools,
    #[document(summary = "Everything the node's operator tuned, which GET /node shows under limits.")]
    pub tuning: crate::node::Tuning,
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
    note = "Routes: GET /status, which includes the fees and the base price a transaction sent now must offer; GET /blocks, newest first, paged with ?before=height&limit=n (20 by default, at most 100); GET /blocks/{block}, where block is latest, a height or a 0x block id; GET /blocks/range?from=height&entry=n&count=n, blocks with what checks them and proofs of the state entries they changed, in binary, a BlockRange; GET /accounts/{address}; GET /accounts/{address}/multisigs, the multisigs its key is a member of, on a node started with --index-multisigs; GET /contracts/{address}; GET /txs/{id}, a transaction's block, height, confirmations, whether it is final, result and events once it is on the main chain, 404 before; GET /validators, every registered validator and the current epoch's set; GET /validators/{voter}; GET /evidence, conflicting votes this node has seen, encoded for Tx::Slash; POST /txs with {\"tx\": \"0x…\"}, a SignedTx encoded as the chain encodes it, which answers 202 with its id; POST /offer with the same body, signed with any offer, which answers the fee offer sure to cover it now; GET /forks, the side branches; GET /events, indexed events, on a node started with --index-events; GET /tokens, the token registry by symbol; GET /collections, the NFT and multi-token collections by symbol; GET /node and GET /mempool, from the running node; GET /stream, server-sent events from a node started with --stream; GET /faucet and POST /faucet, on a node run with --faucet-key. Addresses are written as their kind and hex, such as ed25519:0x3b6a…. Amounts, and integers that can pass 2^53, are decimal strings; hashes are 0x hex. Errors are {\"error\": message} with status 400, 404, 409 for a nonce already used or waiting, or a replacement that does not raise its prices enough, 429 for a faucet asked again too soon or a sender with as many transactions waiting as the node keeps, 500, or 503 when the node cannot take transactions.",
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
    #[cfg(any(feature = "governance", feature = "anchoring"))]
    let whole = Arc::new(genesis.clone());
    let vm = Arc::new(
        Vm::with_pools(&genesis.engines, genesis.hash, options.pools)
            .map_err(|engine| std::io::Error::other(format!("this node cannot run {} contracts", engine.name())))?
            .with_names(genesis.names.clone())
            .with_federation(genesis.federation.clone())
                .with_anchoring(genesis.anchoring.clone())
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

                let trusted_proxies = proxies::TrustedProxies::new(options.trusted_proxies);

                if trusted_proxies.named() {
                    tokio::spawn(trusted_proxies.clone().keep_looking());
                }

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
                    reward: crate::chain::RewardSchedule { genesis: reward, decided: None },
                    built_fees: Arc::default(),
                    vote_rewards,
                    genesis_bootnodes,
                    #[cfg(any(feature = "governance", feature = "anchoring"))]
                    genesis: whole,
                    #[cfg(feature = "faucet")]
                    faucet: options.faucet.map(|config| Arc::new(faucet::Faucet::new(config))),
                    #[cfg(feature = "swap")]
                    swap_contract: options.swap_contract,
                    #[cfg(feature = "dex")]
                    dex_contract: options.dex_contract,
                    #[cfg(feature = "dex")]
                    dex_router: options.dex_router,
                    #[cfg(feature = "updates")]
                    releases: options.releases,
                    in_flight: Arc::new(InFlight { per_ip, counts: Default::default() }),
                    ranges: RangeGate::new((threads / 2).max(1), options.limits.range_wait, options.limits.range_per_ip.max(1)),
                    mining_public: options.mining_public.map(Arc::new),
                    mining_onion: options.mining_onion.map(Arc::new),
                    sharing: options.sharing,
                    report_token: options.report_token.map(Arc::new),
                    trusted_proxies,
                    mining_relays: Arc::new(InFlight { per_ip: options.limits.relays_per_ip.max(1), counts: Default::default() }),
                    tuned: options.limits,
                    tuning: Arc::new(options.tuning),
                    rate: Arc::new(Rate::new(options.limits.rate)),
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
        .route("/blocks/range", get(blocks_range))
        .route("/blocks/{block}", get(block))
        .route("/accounts/{address}", get(account))
        .route("/accounts/{address}/txs", get(account_txs))
        .route("/accounts/{address}/multisigs", get(account_multisigs))
        .route("/contracts/{address}", get(contract))
        .route("/light", get(light))
        .route("/light/updates/{block}", get(light_update))
        .route("/light/finality", get(light_finality))
        .route("/light/bundle", get(light_bundle))
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
        .route("/multisig/{address}", get(multisig))
        .route("/signers", get(signers))
        .route("/evidence", get(evidence))
        .route("/forks", get(forks))
        .route("/events", get(events))
        .route("/tokens", get(tokens))
        .route("/collections", get(collections))
        .route("/supply", get(supply))
        .route("/rich", get(rich))

        .route("/node", get(node))
        .route("/node/unban", post(unban))
        .route("/mempool", get(mempool))
        .route("/offer", post(offer))
        .route("/call", post(call));
    #[cfg(feature = "governance")]
    let router = router
        .route("/governance", get(governance))
        .route("/governance/history", get(governance_history))
        .route("/governance/rejections", get(governance_rejections))
        .route("/governance/weight/{address}", get(governance_weight));
    #[cfg(feature = "ethereum")]
    let router = router.route("/rpc", post(ethereum_rpc));
    #[cfg(feature = "faucet")]
    let router = router.route("/faucet", get(faucet::about).post(faucet::give));
    #[cfg(feature = "names")]
    let router = router.route("/names", get(names)).route("/names/list", get(names_list)).route("/names/{name}", get(name)).route("/accounts/{address}/names", get(account_names));
    #[cfg(feature = "stream")]
    let router = router.route("/stream", get(stream));
    #[cfg(feature = "updates")]
    let router = router.route("/release", get(release));
    #[cfg(feature = "anchoring")]
    let router = router.route("/anchors", get(anchors));
    let router = router.route("/claims/{pool}/{address}", get(claims)).route("/mining", get(mining::about)).route(mining::SOCKET_PATH, get(mining::socket));
    #[cfg(feature = "pool")]
    let router = router.route("/pool", get(pool)).route("/pool/{address}", get(pool_miner)).route("/pool/claim/{address}", get(pool_claim));
    // Bodies are held to body_cap in per_client, which follows the chain's limits, rather than to one
    // size fixed as the router is built.
    let router = router.fallback(apps).layer(DefaultBodyLimit::disable()).layer(middleware::from_fn_with_state(api.clone(), per_client));

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
            headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type, authorization"));
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
    /// Asked again too soon, or a sender with as many transactions waiting as the node keeps.
    TooMany(String),
    /// The transaction's nonce is used or already waiting, and it does not pay enough more to
    /// replace the one waiting.
    Conflict(String),
    /// The node cannot take transactions right now.
    Unavailable(String),
    /// Only the node's operator may: from this machine, or with --report-token.
    Forbidden(String),
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
            ApiError::Forbidden(message) => (StatusCode::FORBIDDEN, message),
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
                "type": crate::registry::changes::Changes::new(&State::new(db.clone())).finality_in(config.epoch_of(tip.header.height), config)?.name(),
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

    // What governance, or the directory's admin, announced: it wins over the operator's flags.
    let directory = crate::registry::changes::Changes::new(&State::new(db.clone())).directory()?;

    // Where vote rewards come from, and what the pool holds when they come from it.
    let decided = crate::registry::changes::Changes::new(&State::new(db.clone())).fees()?.and_then(|fees| fees.vote_rewards);
    let vote_rewards = match decided.unwrap_or(api.vote_rewards) {
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
        "directory": directory.as_ref().map(crate::cli::genesis::directory_json),
        "dex_fee_bps": api.rules.vm.dex.fee_bps,
        "evm_spec": api.rules.vm.evm_spec.name(),
        "directory_admin": crate::registry::directory::directory_admin(&State::new(db.clone()))?.map(|admin| admin.to_string()),
        "upgrade": crate::registry::changes::Changes::new(&State::new(db.clone())).upgrade()?.map(|upgrade| json!({ "least": upgrade.least.to_string(), "from": upgrade.from })),
        "version": crate::structs::version::Version::ours().to_string(),
        "name": crate::registry::changes::Changes::new(&State::new(db.clone())).name()?,
        "collections": api.rules.standards_in(&State::new(db.clone()))?.collections.as_ref().map(crate::cli::genesis::collections_json),
        "finality": finality,
        "fees": fees,
        "vote_rewards": vote_rewards,
        "max_block_bytes": api.max_block_bytes,
        "bootnodes": api.bootnodes(&State::new(db.clone()))?,
        "faucet": faucet_json(&api),
        "swap": swap_json(&api, directory.as_ref()),
        "dex": dex_json(&api, directory.as_ref()),
        "operator": operator_json(&api),
    })))
}

/// The newest release the node took, as signed, for the apps and workers to check themselves; where
/// this node stands against it; and the upgrade that stops it first, governance's or the release's.
#[cfg(feature = "updates")]
async fn release(Shared(api): Shared<Api>) -> Answer {
    use crate::release::Standing;
    use crate::structs::version::Version;

    let ours = Version::ours();
    let known = api.releases.as_ref().map(|releases| releases.known()).unwrap_or_default();
    let genesis = at_height(&*api.db, 0)?.id().to_bytes();
    let decided = Changes::new(&State::new(api.db.clone())).upgrade()?.filter(|upgrade| upgrade.stops(ours));
    let announced = known.signed.as_ref().and_then(|signed| signed.release.stopping(&genesis, ours));
    let upgrade = match (decided, announced) {
        (Some(decided), Some(announced)) if announced.from < decided.from => Some((announced, "release")),
        (Some(decided), _) => Some((decided, "governance")),
        (None, announced) => announced.map(|announced| (announced, "release")),
    };
    let now = crate::node::now_ms() / 1_000;
    let (standing, newest) = match known.standing(now) {
        Some(Standing::Current) => ("current", None),
        Some(Standing::Behind { newest }) => ("behind", Some(newest)),
        Some(Standing::Unsupported { newest, .. }) => ("unsupported", Some(newest)),
        None => ("unknown", None),
    };

    Ok(Json(json!({
        "version": ours.to_string(),
        "target": crate::release::TARGET,
        "updates": api.releases.as_ref().is_some_and(|releases| releases.keys().on()),
        "manifest": known.signed.as_ref().map(crate::release::SignedRelease::to_json),
        "standing": standing,
        "expired": known.expired(now),
        "newest": newest.map(|newest| newest.to_string()),
        "checked": known.checked,
        "error": known.error,
        "installed": known.installed.map(|installed| installed.to_string()),
        "upgrade": upgrade.map(|(upgrade, source)| json!({ "least": upgrade.least.to_string(), "from": upgrade.from, "source": source })),
    })))
}

/// The DEXes clients use: { contract, router }, each the directory's, or what the node's
/// operator named where the directory names none, and null when neither does; null when neither
/// names either.
fn dex_json(api: &Api, directory: Option<&crate::registry::directory::Directory>) -> Value {
    let (contract, router) = (directory.and_then(|directory| directory.dex), directory.and_then(|directory| directory.dex_router));
    #[cfg(feature = "dex")]
    let (contract, router) = (contract.or(api.dex_contract), router.or(api.dex_router));
    let _ = api;

    match (contract, router) {
        (None, None) => Value::Null,
        (contract, router) => json!({
            "contract": contract.map(|contract| contract.to_string()),
            "router": router.map(|router| to_hex(&router)),
        }),
    }
}

/// The swap contract clients use: { contract }, the directory's, or what the node's operator
/// named when the directory names none; null when neither does.
fn swap_json(api: &Api, directory: Option<&crate::registry::directory::Directory>) -> Value {
    let contract = directory.and_then(|directory| directory.swap);
    #[cfg(feature = "swap")]
    let contract = contract.or(api.swap_contract);
    let _ = api;

    contract.map_or(Value::Null, |contract| json!({ "contract": contract.to_string() }))
}

/// What the node's operator named with --swap-contract, --dex-contract and --dex-router, as given,
/// each null when not: a suggestion that fills only what the directory leaves unset, shown apart so
/// a client sees where its contracts came from. Null for a node given none.
fn operator_json(api: &Api) -> Value {
    #[allow(unused_mut)]
    let mut named = serde_json::Map::new();

    #[cfg(feature = "swap")]
    if let Some(contract) = api.swap_contract {
        named.insert("swap".to_string(), contract.to_string().into());
    }

    #[cfg(feature = "dex")]
    {
        if let Some(contract) = api.dex_contract {
            named.insert("dex".to_string(), contract.to_string().into());
        }

        if let Some(router) = api.dex_router {
            named.insert("dex_router".to_string(), to_hex(&router).into());
        }
    }

    let _ = api;

    match named.is_empty() {
        true => Value::Null,
        false => Value::Object(named),
    }
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

    let set = config.voters_in(&state, epoch)?.unwrap_or_default();

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

/// A multisig: its policy, its next nonce and balance, and the proposals open for that nonce,
/// each with the members that approve it.
async fn multisig(Shared(api): Shared<Api>, Path(address): Path<String>) -> Answer {
    let address: Address = address.parse().map_err(|_| ApiError::BadRequest(format!("not an address: {address}")))?;
    let state = State::new(api.db);
    let multisigs = crate::registry::Multisigs::new(&state);
    let policy = multisigs.get(&address)?.ok_or_else(|| ApiError::NotFound(format!("{address} is not a multisig")))?;
    let account = crate::registry::Accounts::new(&state).get(&address)?;
    let open = multisigs.proposals(&address, account.nonce)?;

    Ok(Json(json!({
        "address": address.to_string(),
        "members": policy.members.iter().map(|member| json::multisig_member(member, policy.testnet)).collect::<Vec<_>>(),
        "threshold": policy.threshold,
        "testnet": policy.testnet,
        "bitcoin": bitcoin_side(&policy),
        "evm": evm_side(&address, &policy, multisigs.evm_start(&address)?),
        "nonce": account.nonce,
        "balance": account.balance.to_string(),
        "proposals": open.proposals.iter().map(|proposal| json!({
            "id": proposal.id.to_string(),
            "tx": json::tx(&proposal.tx),
            "encoded": to_hex(&wincode::serialize(&proposal.tx).expect("transactions always encode")),
            "fee": json::fee_offer(&proposal.fee),
            "approvals": proposal.approvals.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })))
}

/// A multisig's descriptors on Bitcoin, receiving and change, and its first receiving address,
/// when its members name xpubs.
fn bitcoin_side(policy: &crate::registry::multisig::MultisigPolicy) -> Value {
    #[cfg(feature = "bitcoin")]
    if let Some(xpubs) = policy.members.iter().map(|member| member.bitcoin).collect::<Option<Vec<_>>>() {
        use crate::crypto::bitcoin::multisig::{bitcoin_address, descriptor};

        let network = json::network(policy.testnet);

        return json!({
            "network": network.name(),
            "receiving": descriptor(policy.threshold, &xpubs, network, 0),
            "change": descriptor(policy.threshold, &xpubs, network, 1),
            "first_address": bitcoin_address(policy.threshold, &xpubs, network, 0, 0),
        });
    }

    let _ = policy;

    Value::Null
}

/// A multisig's contract on EVM chains: its address, the owners and threshold it starts with, and
/// the ones its members name now, which its owners should be; null while they name no EVM
/// addresses.
fn evm_side(address: &Address, policy: &crate::registry::multisig::MultisigPolicy, start: Option<crate::registry::multisig::EvmStart>) -> Value {
    #[cfg(all(feature = "abi", feature = "secp256k1"))]
    if let Some(start) = start {
        let owners = |start: &crate::registry::multisig::EvmStart| json!({
            "owners": start.owners.iter().map(|owner| to_hex(owner)).collect::<Vec<_>>(),
            "threshold": start.threshold,
        });

        return json!({
            "contract": to_hex(&crate::abi::multisig::multisig_contract(address, &start)),
            "start": owners(&start),
            "now": policy.evm().as_ref().map(owners),
        });
    }

    let _ = (address, policy, start);

    Value::Null
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
/// Governance: the consensus the tip's epoch runs and the one decided last, who may vote and what
/// each weighs, and the votes that still count in the tip's epoch, each change with its tally:
/// what Yes, No and Abstain weigh, what Yes needs, the quorum, and whether it can still pass.
async fn governance(Shared(api): Shared<Api>) -> Answer {
    let Some(genesis_rules) = &api.rules.governance else {
        return Err(ApiError::NotFound("this chain has no governance".to_string()));
    };
    let db = api.db.clone();
    let height = tip(&*db)?.header.height;
    let epoch = genesis_rules.finality.epoch_of(height);
    let state = State::new(db);
    let rules = genesis_rules.in_state(&state)?;
    let changes = Changes::new(&state);
    let decided = changes.decided()?;
    let needed = rules.governance.needed(&state, epoch)?;
    let total = rules.governance.total(&state, epoch)?;
    let voters = rules.governance.voters(&state, epoch)?;
    let mut proposals = Vec::new();
    let mut versions = serde_json::Map::new();

    for kind in crate::governance::ChangeKind::ALL {
        versions.insert(kind.name().to_string(), changes.version_of(kind)?.into());
    }

    // Each kind's ballot, in kind order: a change's votes are of its kind, at its version.
    for (kind, ballot) in changes.ballots()? {
        let ballot = ballot.counting(epoch, rules.approval_epochs);

        for change in ballot.changes() {
            let mut votes = Vec::new();

            for cast in ballot.votes_on(&change) {
                votes.push(json!({
                    "account": cast.account.to_string(),
                    "vote": cast.vote.name(),
                    "weight": rules.governance.weight(&state, epoch, &cast.account)?.unwrap_or_default().to_string(),
                    "epoch": cast.epoch,
                    "counts_until": cast.epoch + rules.approval_epochs - 1,
                }));
            }

            let tally = changes.tally(&ballot, &change, epoch, rules.quorum_permille, &*rules.governance)?;
            let mut value = json::change(&change);
            value["kind"] = kind.name().into();
            value["version"] = ballot.version.into();
            value["tally"] = tally_json(&tally);
            // The first epoch a vote on it stops counting in, as old votes run out.
            value["closes"] = ballot.votes_on(&change).map(|cast| cast.epoch + rules.approval_epochs - 1).min().into();
            value["votes"] = votes.into();
            proposals.push(value);
        }
    }

    Ok(Json(json!({
        "kind": rules.governance.name(),
        "epoch": epoch,
        "checkpoint_interval": genesis_rules.checkpoint_interval,
        "active": changes.consensus_for(epoch)?.as_ref().map(|consensus| crate::cli::genesis::consensus_json(consensus, api.light.pow_window)),
        "version": decided.version,
        "versions": versions,
        "rejections": changes.rejections()?,
        "decided": crate::cli::genesis::consensus_json(&decided.consensus, api.light.pow_window),
        "approval_epochs": rules.approval_epochs,
        "quorum_permille": rules.quorum_permille,
        "needed": needed.to_string(),
        "total": total.to_string(),
        "bootnodes": api.bootnodes(&state)?,
        "finality": changes.finality_in(epoch, &genesis_rules.finality)?.name(),
        "switch": changes.switch_ahead(epoch, &genesis_rules.finality)?.map(|era| json!({ "epoch": era.from, "type": era.config.name() })),
        "proposals": proposals,
        "members": rules.governance.members().map(|members| members.iter().map(Address::to_string).collect::<Vec<_>>()),
        "voters": voters.iter().map(|(account, weight)| json!({ "account": account.to_string(), "weight": weight.to_string() })).collect::<Vec<_>>(),
        "in_force": in_force(&api, &state, epoch, decided.version)?,
        "locks": changes.locks()?,
    })))
}

#[cfg(feature = "governance")]
/// A change's tally as JSON: weights as decimal strings, beside what they come to.
fn tally_json(tally: &crate::registry::changes::Tally) -> Value {
    json!({
        "yes": tally.yes.to_string(),
        "no": tally.no.to_string(),
        "abstain": tally.abstain.to_string(),
        "voted": tally.voted().to_string(),
        "total": tally.total.to_string(),
        "needed": tally.needed.to_string(),
        "quorum": tally.quorum.to_string(),
        "more_needed": tally.more_needed().to_string(),
        "quorum_met": tally.voted() >= tally.quorum,
        "can_pass": tally.can_pass(),
    })
}

#[cfg(feature = "governance")]
/// Each setting governance may change, as it stands in the tip's state, under its kind: genesis's
/// while governance never changed it, with decided naming the version that last did, or null.
fn in_force(api: &Api, state: &State, epoch: u64, newest: u64) -> Result<Value, ApiError> {
    let rules = api.rules.governance.as_ref().expect("only a governed chain asks");
    // What the chain compares a change with to hold its locks, the same list.
    let settings = Changes::new(state).in_force(epoch, &rules.genesis, &rules.finality, &rules.in_state(state)?.config)?;
    // The version that last changed each kind, from the decisions this node indexed.
    let mut last = HashMap::new();

    for (version, id) in stored_decisions(&*api.db, newest, None, usize::MAX)? {
        if let Some(change) = decided_change(&*api.db, &id)? {
            last.entry(change.name()).or_insert(version);
        }
    }

    let mut value = json!({});

    for change in settings {
        let kind = change.name();
        let mut setting = json!({ "value": json::change(&change).as_object().and_then(|change| change.values().next().cloned()).unwrap_or(Value::Null) });
        // A switch of finality's kind is decided as a change of its own, of the same setting.
        setting["decided"] = match kind {
            "finality" => last.get("finality").max(last.get("switch_finality")).copied(),
            kind => last.get(kind).copied(),
        }
        .into();
        value[kind] = setting;
    }

    Ok(value)
}

#[cfg(feature = "governance")]
/// The change a decision's or rejection's vote was on: its own, or the one a multisig cast for it.
fn decided_change(db: &dyn Committed, id: &Hash) -> Result<Option<crate::governance::Change>, ApiError> {
    use crate::structs::tx::Tx;
    let Some(receipt) = stored_receipt(db, id)? else { return Ok(None) };
    let Some(block) = stored_block(db, &receipt.block)? else { return Ok(None) };
    let Some(signed) = block.txs.get(receipt.index as usize) else { return Ok(None) };
    let mut tx = &signed.tx;

    loop {
        match tx {
            Tx::VoteChange { change, .. } => return Ok(Some(change.clone())),
            Tx::ApproveMultisig { tx: inner, .. } => tx = inner,
            _ => return Ok(None),
        }
    }
}

/// The coin's supply at the tip: what was made and destroyed since genesis, which a light client
/// proves with the state entry under key, and what is bonded and in the reward pool, which the
/// node reports.
async fn supply(Shared(api): Shared<Api>) -> Answer {
    use crate::registry::supply::{SUPPLY_KEY, Supply};

    let db = api.db.clone();
    let height = tip(&*db)?.header.height;
    let state = State::new(db);
    let counted = Supply::of(&state)?;
    #[cfg(feature = "staking")]
    let staked = {
        let validators = crate::registry::validators::Validators::new(&state);
        let mut staked = u256::ZERO;

        for voter in validators.voters()? {
            if let Some(record) = validators.get(&voter).map_err(|err| ApiError::Db(DbError::Corrupt(err.to_string())))? {
                staked = staked.saturating_add(record.bond);
            }
        }

        staked
    };
    #[cfg(not(feature = "staking"))]
    let staked = u256::ZERO;

    Ok(Json(json!({
        "height": height,
        "minted": counted.minted.to_string(),
        "burned": counted.burned.to_string(),
        "supply": counted.supply().to_string(),
        "staked": staked.to_string(),
        "reward_pool": Accounts::new(&state).get(&REWARD_POOL)?.balance.to_string(),
        "key": to_hex(SUPPLY_KEY),
    })))
}

/// The accounts that hold the most coin at the tip, richest first, at most ?limit, and how many
/// hold any: a scan of every account, which the node reports and nothing proves as a list.
async fn rich(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let limit = page(&api.tuned, &query, DEFAULT_PAGE, MAX_PAGE)?.max(1) as usize;
    let db = api.db.clone();
    let height = tip(&*db)?.header.height;
    let mut richest: Vec<(u256, Address)> = Vec::new();
    let mut holders = 0u64;

    db.scan(Column::Accounts, &[], &[], &mut |key, value| {
        if let Some((address, account)) = crate::registry::accounts::account_entry(key, value)
            && account.balance > u256::ZERO
        {
            holders += 1;
            richest.push((account.balance, address));

            // Kept to the page, so a chain of many accounts holds no more than that.
            if richest.len() > 2 * limit {
                richest.sort_unstable_by(|a, b| b.cmp(a));
                richest.truncate(limit);
            }
        }

        true
    })?;

    richest.sort_unstable_by(|a, b| b.cmp(a));
    richest.truncate(limit);

    Ok(Json(json!({
        "height": height,
        "holders": holders,
        "accounts": richest.iter().map(|(balance, address)| json!({ "address": address.to_string(), "balance": balance.to_string() })).collect::<Vec<_>>(),
    })))
}

/// The anchor list: anchoring's settings in force, how many anchors the chain recorded, the list's
/// root, the highest enforced checkpoint, the Bitcoin relay's tip, the Ethereum relay's finalized
/// header (eth_finalized: its slot, root, and execution block's number and hash), and a page of entries, the newest last,
/// from ?from on, the newest page unless told. Each entry names its chain, bitcoin or ethereum, and
/// where that chain holds it: btc_block and btc_height, or eth_root and eth_slot.
#[cfg(feature = "anchoring")]
async fn anchors(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let state = State::new(api.db.clone());
    let settings = Changes::new(&state).anchoring()?.or_else(|| api.genesis.anchoring.clone()).unwrap_or_default();
    let anchors = crate::anchor::Anchors::new(&state);
    let count = anchors.count()?;
    let limit = page(&api.tuned, &query, DEFAULT_PAGE, MAX_PAGE)?;
    let from = number(&query, "from")?.unwrap_or_else(|| count.saturating_sub(limit));
    let shown = |hash: &[u8; 32]| crate::structs::hex::to_hex(&hash.iter().rev().copied().collect::<Vec<_>>());
    let entries: Vec<Value> = anchors
        .entries(from, limit)?
        .iter()
        .zip(from..)
        .map(|(entry, index)| {
            let mut value = json!({
                "index": index,
                "epoch": entry.epoch,
                "block": entry.block.to_string(),
                "chain": entry.source.chain().name(),
                "enforced": entry.enforced,
            });

            match entry.source {
                crate::params::anchoring::AnchorSource::Bitcoin { block, height } => {
                    value["btc_block"] = shown(&block).into();
                    value["btc_height"] = height.into();
                }
                crate::params::anchoring::AnchorSource::Ethereum { root, slot } => {
                    value["eth_root"] = crate::structs::hex::to_hex(&root).into();
                    value["eth_slot"] = slot.into();
                }
            }

            value
        })
        .collect();
    #[cfg(feature = "anchoring-btc")]
    let relay_tip = match settings.relay {
        Some(relay) => crate::anchor::Relay::new(&state, relay).tip()?.map(|(hash, height)| json!({ "hash": shown(&hash), "height": height })),
        None => None,
    };
    #[cfg(not(feature = "anchoring-btc"))]
    let relay_tip: Option<Value> = None;
    #[cfg(feature = "anchoring-eth")]
    let eth_finalized = match settings.eth_relay {
        Some(relay) => crate::anchor::EthRelay::new(&state, relay).finalized()?.map(|header| {
            json!({ "slot": header.beacon.slot, "root": crate::structs::hex::to_hex(&header.root), "block_number": header.block_number, "block_hash": crate::structs::hex::to_hex(&header.block_hash) })
        }),
        None => None,
    };
    #[cfg(not(feature = "anchoring-eth"))]
    let eth_finalized: Option<Value> = None;

    Ok(Json(json!({
        "settings": crate::params::json::anchoring_json(&settings),
        "chain_tag": crate::structs::hex::to_hex(&crate::anchor::chain_tag(&api.genesis.block().id())),
        "count": count,
        "root": crate::structs::hex::to_hex(&anchors.root()?),
        "enforced_height": anchors.enforced_height()?,
        "rewarded": anchors.rewarded()?,
        "relay_tip": relay_tip,
        "eth_finalized": eth_finalized,
        "from": from,
        "entries": entries,
    })))
}

#[cfg(feature = "governance")]
/// Governance's decisions, newest first, below the version in ?before: each the version it made,
/// the approval that decided it, its block, who sent it and the change. A light client proves each
/// with GET /proofs/receipts/{tx}, whose outcome is Decided at that version; versions count up
/// one at a time, so a gap shows. from is the oldest this node keeps, 1 unless it synced from a
/// snapshot.
async fn governance_history(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    if api.rules.governance.is_none() {
        return Err(ApiError::NotFound("this chain has no governance".to_string()));
    }

    let db = api.db.clone();
    let tip = tip(&*db)?.header.height;
    let newest = Changes::new(&State::new(db.clone())).decided()?.version;
    let limit = page(&api.tuned, &query, DEFAULT_PAGE, MAX_PAGE)? as usize;
    let mut decisions = Vec::new();

    for (version, id) in stored_decisions(&*db, newest, number(&query, "before")?, limit)? {
        let Some(receipt) = stored_receipt(&*db, &id)? else { continue };
        let block = stored_block(&*db, &receipt.block)?.ok_or_else(|| ApiError::Db(DbError::Corrupt(format!("block {} is not stored", receipt.block))))?;
        let sender = block.txs.get(receipt.index as usize).map(|signed| signed.sender().to_string());
        let mut value = json!({
            "version": version,
            "tx": id.to_string(),
            "block": receipt.block.to_string(),
            "height": block.header.height,
            "timestamp": block.header.timestamp,
            "confirmations": tip.saturating_sub(block.header.height) + 1,
            "final": block.header.height <= finalized_height(&*db)?,
            "sender": sender,
        });

        if let crate::chain::TxStatus::Decided { kind, version, .. } = receipt.status {
            value["kind_version"] = version.into();
            value["of"] = kind.name().into();
        }

        if let Some(change) = decided_change(&*db, &id)? {
            value["kind"] = change.name().into();
            value["change"] = json::change(&change);
        }

        decisions.push(value);
    }

    // The oldest of the unbroken run down from the newest: what this node can list at all.
    let from = stored_decisions(&*db, newest, None, usize::MAX)?.last().map_or(newest + 1, |(version, _)| *version);

    Ok(Json(json!({ "version": newest, "from": from, "decisions": decisions })))
}

#[cfg(feature = "governance")]
/// Governance's rejections, newest first, below the count in ?before: each the chain's count it
/// made, the vote that rejected it, its block, who sent it and the change. A light client proves
/// each with GET /proofs/receipts/{tx}, whose outcome is Rejected at that count; counts go up one
/// at a time, so a gap shows. from is the oldest this node keeps.
async fn governance_rejections(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    if api.rules.governance.is_none() {
        return Err(ApiError::NotFound("this chain has no governance".to_string()));
    }

    let db = api.db.clone();
    let tip = tip(&*db)?.header.height;
    let newest = Changes::new(&State::new(db.clone())).rejections()?;
    let limit = page(&api.tuned, &query, DEFAULT_PAGE, MAX_PAGE)? as usize;
    let mut rejections = Vec::new();

    for (rejection, id) in crate::chain::stored_rejections(&*db, newest, number(&query, "before")?, limit)? {
        let Some(receipt) = stored_receipt(&*db, &id)? else { continue };
        let block = stored_block(&*db, &receipt.block)?.ok_or_else(|| ApiError::Db(DbError::Corrupt(format!("block {} is not stored", receipt.block))))?;
        let sender = block.txs.get(receipt.index as usize).map(|signed| signed.sender().to_string());
        let mut value = json!({
            "rejection": rejection,
            "tx": id.to_string(),
            "block": receipt.block.to_string(),
            "height": block.header.height,
            "timestamp": block.header.timestamp,
            "confirmations": tip.saturating_sub(block.header.height) + 1,
            "final": block.header.height <= finalized_height(&*db)?,
            "sender": sender,
        });

        if let crate::chain::TxStatus::Rejected { kind, version, .. } = receipt.status {
            value["kind_version"] = version.into();
            value["of"] = kind.name().into();
        }

        if let Some(change) = decided_change(&*db, &id)? {
            value["kind"] = change.name().into();
            value["change"] = json::change(&change);
        }

        rejections.push(value);
    }

    let from = crate::chain::stored_rejections(&*db, newest, None, usize::MAX)?.last().map_or(newest + 1, |(rejection, _)| *rejection);

    Ok(Json(json!({ "rejection": newest, "from": from, "rejections": rejections })))
}

#[cfg(feature = "governance")]
/// What an account's vote weighs in the tip's epoch, beside what a change needs and what every
/// voter weighs, and its votes that still count: zero for an account that may not vote.
async fn governance_weight(Shared(api): Shared<Api>, Path(address): Path<String>) -> Answer {
    let Some(rules) = &api.rules.governance else {
        return Err(ApiError::NotFound("this chain has no governance".to_string()));
    };
    let address = parse_address(&address)?;
    let db = api.db.clone();
    let epoch = rules.finality.epoch_of(tip(&*db)?.header.height);
    let state = State::new(db);
    let rules = rules.in_state(&state)?;
    let changes = Changes::new(&state);
    let mut voted = Vec::new();

    // Its votes in each kind's ballot that still count: one a change, one Yes a kind at most.
    for (kind, ballot) in changes.ballots()? {
        let ballot = ballot.counting(epoch, rules.approval_epochs);

        for cast in ballot.votes.iter().filter(|cast| cast.account == address) {
            voted.push(json!({
                "kind": kind.name(),
                "version": ballot.version,
                "vote": cast.vote.name(),
                "epoch": cast.epoch,
                "counts_until": cast.epoch + rules.approval_epochs - 1,
                "change": json::change(&cast.change),
            }));
        }
    }

    Ok(Json(json!({
        "account": address.to_string(),
        "weight": rules.governance.weight(&state, epoch, &address)?.unwrap_or_default().to_string(),
        "needed": rules.governance.needed(&state, epoch)?.to_string(),
        "total": rules.governance.total(&state, epoch)?.to_string(),
        "voted": voted,
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
    let limit = page(&api.tuned, &query, DEFAULT_PAGE, MAX_PAGE)?;
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

/// Blocks from ?from= on, at most ?count=, each with what recomputes its header's roots and proofs
/// of the state entries it changed, a BlockRange, encoded; ?entry= goes on through a block's entries
/// as the answer before said to. Each is built in its turn, on the blocking pool, where it holds up
/// no other request, and within the limits it serves ranges within, RangeLimits::SERVED by default.
async fn blocks_range(Shared(api): Shared<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>) -> Result<Response, ApiError> {
    let from = number(&query, "from")?.ok_or_else(|| ApiError::BadRequest("from is the height to start at".to_string()))?;
    let entry = u32::try_from(number(&query, "entry")?.unwrap_or(0)).map_err(|_| ApiError::BadRequest("entry is too big".to_string()))?;
    let served = api.tuned.range;
    let count = number(&query, "count")?.unwrap_or(served.blocks);
    let turn = match api.ranges.enter(asker(peer, &headers, &api.trusted_proxies)).await {
        Ok(turn) => turn,
        // As nginx's own 429 does, it says when to come back.
        Err(busy) => {
            let mut response = busy.into_response();
            response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));

            return Ok(response);
        }
    };
    let (db, hash) = (api.db, api.hash);
    // Held by the work, not this request: a client that hangs up does not stop what it started.
    let range = tokio::task::spawn_blocking(move || {
        let _turn = turn;

        Ok::<_, DbError>(match stored_block_range(db, hash, from, entry, count, &served)? {
            Ok(range) => Ok(wincode::serialize(&range).map_err(|err| DbError::Corrupt(format!("a block range: {err}")))?),
            Err(problem) => Err(problem),
        })
    })
    .await
    .map_err(|err| ApiError::Db(DbError::Corrupt(format!("a block range stopped: {err}"))))??;

    match range {
        Ok(bytes) => Ok(([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response()),
        Err(problem @ RangeProblem::NotHeld) => Err(ApiError::NotFound(format!("height {from}: {problem}"))),
        Err(problem @ RangeProblem::NoSuchEntry) => Err(ApiError::BadRequest(format!("height {from}: {problem}"))),
    }
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

    // A pool's block: what it shared out, what went to the escrow, and whose escrow it is.
    if let Some(pool) = &found.pool {
        let shared = stored_split(&*db, &id)?;
        value["pool"]["shared"] = shared.map(|shared| shared.to_string()).into();
        value["pool"]["escrowed"] = shared.map(|shared| crate::structs::block::parts_of(shared, pool.parts).to_string()).into();
        value["pool"]["escrow"] = crate::registry::pools::pool_escrow(&found.header.producer, hash).to_string().into();
    }

    Ok(Json(value))
}

/// The pool this node runs: its fee and window, every miner with its work in the window, what the
/// next block gives it and all it has earned, and the blocks it sealed lately, each with whether
/// it was credited and what each miner's part came to. With an address, that miner alone, with
/// what it claimed from the escrow.
#[cfg(feature = "pool")]
async fn pool(Shared(api): Shared<Api>) -> Answer {
    pool_for(&api, None, api.sharing).await
}

#[cfg(feature = "pool")]
async fn pool_miner(Shared(api): Shared<Api>, Path(address): Path<String>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Answer {
    let (sharing, _) = api.shared_with(peer, &headers);

    pool_for(&api, Some(parse_address(&address)?), sharing).await
}

/// The pool this node runs, from its report, or a 404 for a node that runs none.
#[cfg(feature = "pool")]
async fn pool_report(api: &Api) -> Result<crate::consensus::PoolReport, ApiError> {
    report(api, 0)
        .await?
        .mining
        .and_then(|mining| mining.pool)
        .ok_or_else(|| ApiError::NotFound("this node runs no mining pool".to_string()))
}

#[cfg(feature = "pool")]
async fn pool_for(api: &Api, miner: Option<crate::structs::address::Address>, sharing: sharing::Sharing) -> Answer {
    use crate::registry::pools::{Pools, pool_escrow};

    let report = report(api, 0).await?;
    let pool = report.mining.as_ref().and_then(|mining| mining.pool.clone()).ok_or_else(|| ApiError::NotFound("this node runs no mining pool".to_string()))?;
    let producer = pool.producer.ok_or_else(|| ApiError::Unavailable("the pool does not say its address".to_string()))?;
    let (state, _) = state_at(api, None)?;
    let escrow = pool_escrow(&producer, api.hash);
    let mut blocks = Vec::new();

    for sealed in &pool.blocks {
        let paid = |payout: &&crate::structs::block::Payout| miner.is_none_or(|miner| payout.to == miner);

        if miner.is_some() && !sealed.payouts.iter().any(|payout| paid(&payout)) {
            continue;
        }

        let shared = stored_split(&*api.db, &sealed.id)?;
        let payouts: Vec<_> = sealed.payouts.iter().filter(paid).copied().collect();
        blocks.push(json!({
            "height": sealed.height,
            "id": sealed.id.to_string(),
            "at": sealed.at,
            "credited": sealed.credited,
            "shared": shared.map(|shared| shared.to_string()),
            "payouts": json::payouts(&payouts, shared),
        }));
    }

    let mut miners = Vec::new();

    for standing in pool.miners.iter().filter(|standing| miner.is_none_or(|miner| standing.to == miner)) {
        let claimed = Pools::new(&state, api.hash).claimed(&producer, &standing.to)?;
        miners.push(json!({
            "to": standing.to.to_string(),
            "work": standing.work.to_string(),
            "shares": standing.shares,
            "parts": standing.parts,
            "percent": json::percent(standing.parts),
            "earned": standing.earned.to_string(),
            "claimed": claimed.to_string(),
            "claimable": standing.earned.saturating_sub(claimed).to_string(),
        }));
    }

    let workers: Vec<Value> = report
        .mining
        .iter()
        .flat_map(|mining| &mining.workers)
        .filter(|worker| !sharing.hides(sharing::ReportField::Workers) && miner.is_some_and(|miner| worker.payout == Some(miner)))
        .enumerate()
        .map(|(index, worker)| json!({ "name": json::worker_name(&worker.name, index, sharing), "rate": worker.devices.iter().map(|device| device.rate).sum::<f64>(), "shares": worker.shares, "share_difficulty": worker.share_difficulty.to_string() }))
        .collect();

    Ok(Json(json!({
        "producer": producer.to_string(),
        "escrow": escrow.to_string(),
        "escrow_balance": Accounts::new(&state).get(&escrow)?.balance.to_string(),
        "fee": pool.fee,
        "fee_percent": json::percent(pool.fee),
        "window": pool.window,
        "open": pool.open,
        "min_share": pool.min_share.to_string(),
        "work": pool.work.to_string(),
        "earned": pool.earned.to_string(),
        "miners": miners,
        "workers": workers,
        "blocks": blocks,
    })))
}

/// A miner's claim on the pool this node runs: all it earned, what it claimed, and the Merkle path
/// from its leaf to the newest root a block on the main chain committed to, which Tx::Claim
/// takes as it is. With the height the pool's blocks are settled to, so a miner keeping its
/// claims knows which can no longer be undone. A 404 for a miner the pool owes nothing yet.
#[cfg(feature = "pool")]
async fn pool_claim(Shared(api): Shared<Api>, Path(address): Path<String>) -> Answer {
    use crate::registry::pools::{Pools, pool_escrow};

    let miner = parse_address(&address)?;
    let pool = pool_report(&api).await?;
    let producer = pool.producer.ok_or_else(|| ApiError::Unavailable("the pool does not say its address".to_string()))?;
    let (state, _) = state_at(&api, None)?;
    let pools = Pools::new(&state, api.hash);

    // The newest tree whose root a block on the main chain carries, by its newest such block.
    for tree in &pool.claims {
        for (height, block) in tree.committed.iter().rev() {
            if pools.root(*height)?.is_none_or(|root| root.root != tree.root || root.pool != producer) {
                continue;
            }

            let Some(claim) = tree.claim(api.hash, producer, *height, *block, &miner) else {
                return Err(ApiError::NotFound(format!("the pool owes {miner} nothing yet")));
            };
            let claimed = pools.claimed(&producer, &miner)?;
            let mut value = crate::mining::claims::claim_to_json(&claim);
            value["escrow"] = json!(pool_escrow(&producer, api.hash).to_string());
            value["claimed"] = json!(claimed.to_string());
            value["claimable"] = json!(claim.earned.saturating_sub(claimed).to_string());
            value["settled"] = json!(pool.settled());

            return Ok(Json(value));
        }
    }

    Err(ApiError::NotFound("no block on the main chain carries the pool's claims yet".to_string()))
}

/// What a miner has claimed from a pool, which any node answers, the pool's or not: with the root
/// the pool's block at ?height= committed to, if a block on the main chain there carries one of
/// the pool's, for a claim the miner kept to be checked against before it is sent.
async fn claims(Shared(api): Shared<Api>, Path((pool, address)): Path<(String, String)>, Query(query): Query<HashMap<String, String>>) -> Answer {
    use crate::registry::pools::{Pools, pool_escrow};

    let (pool, miner) = (parse_address(&pool)?, parse_address(&address)?);
    let (state, header) = state_at(&api, None)?;
    let pools = Pools::new(&state, api.hash);
    let escrow = pool_escrow(&pool, api.hash);
    let root = match number(&query, "height")? {
        Some(height) => pools.root(height)?.filter(|root| root.pool == pool),
        None => None,
    };

    Ok(Json(json!({
        "pool": pool.to_string(),
        "to": miner.to_string(),
        "escrow": escrow.to_string(),
        "escrow_balance": Accounts::new(&state).get(&escrow)?.balance.to_string(),
        "claimed": pools.claimed(&pool, &miner)?.to_string(),
        "height": header.height,
        "root": root.map(|root| json!({ "root": root.root.to_string(), "count": root.count })),
    })))
}

/// The side branches this node stores: every block off the main chain, newest first, from each
/// leaf back to where its branch meets the main chain.
async fn forks(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let limit = page(&api.tuned, &query, DEFAULT_FORKS, MAX_FORKS)? as usize;
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

/// Indexed events, newest first: a contract's, a contract's of one name, those that mention an
/// address, or EVM logs with a topic, of a contract or of any, a page at a time. Anything else is
/// refused rather than half answered. The answer says it knows topics: an older node ignores what
/// it does not know, so a client asking one for a contract's logs of a topic gets all of them.
async fn events(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let Some(index) = stored_index(&*db, IndexKind::Events)? else {
        return Err(ApiError::NotFound("this node keeps no event index; start it with --index-events".to_string()));
    };
    let topic = query
        .get("topic")
        .map(|topic| from_hex(topic).and_then(|bytes| <[u8; 32]>::try_from(bytes).ok()).ok_or_else(|| ApiError::BadRequest("a topic is 0x and 64 hex digits".to_string())))
        .transpose()?;
    let filter = match (query.get("contract"), query.get("name"), query.get("address"), topic) {
        (Some(contract), None, None, None) => EventFilter::Contract(parse_address(contract)?),
        (Some(contract), Some(name), None, None) => EventFilter::Named(parse_address(contract)?, name.clone()),
        (None, None, Some(address), None) => EventFilter::Mentions(parse_address(address)?),
        (Some(contract), None, None, Some(topic)) => EventFilter::ContractTopic(parse_address(contract)?, topic),
        (None, None, None, Some(topic)) => EventFilter::Topic(topic),
        _ => {
            return Err(ApiError::BadRequest(
                "ask for a contract's events, with or without a name or a topic; an address's; or a topic's".to_string(),
            ));
        }
    };
    let limit = page(&api.tuned, &query, DEFAULT_EVENTS, MAX_EVENTS)?.max(1) as usize;
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
        "topics": true,
    })))
}

/// How the node behind the API is doing: its peers, what it heard and did, how syncing goes.
async fn node(Shared(api): Shared<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Answer {
    let (sharing, refused) = api.shared_with(peer, &headers);
    let report = report(&api, 0).await?;
    let mut answer = json::node(&report, sharing, &api.tuning);
    answer["token_refused"] = refused.into();

    Ok(Json(answer))
}

/// Lifts bans, the operator's alone: {"all": true}, {"peer": "12D3…"} or {"ip": "203.0.113.7"}. The
/// node lets them go from its database and the network lifts them now.
async fn unban(Shared(api): Shared<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Answer {
    if api.shared_with(peer, &headers).0 != sharing::Sharing::EVERYTHING {
        return Err(ApiError::Forbidden("only the node's operator lifts bans: from this machine, or with --report-token".to_string()));
    }

    let which = unban_of(&json_body(&body)?)?;
    let Some(node) = &api.node else {
        return Err(ApiError::Unavailable("this API has no node behind it".to_string()));
    };
    let lifted = node.unban(which).await.map_err(|err| ApiError::Unavailable(err.to_string()))?;

    Ok(Json(json!({ "lifted": lifted })))
}

/// Which bans a POST /node/unban names: all, one peer's or one IP's.
fn unban_of(body: &Value) -> Result<crate::net::Unban, ApiError> {
    use crate::net::Unban;

    match (body.get("all").and_then(Value::as_bool), body.get("peer").and_then(Value::as_str), body.get("ip").and_then(Value::as_str)) {
        (Some(true), None, None) => Ok(Unban::All),
        (None, Some(peer), None) => peer.trim().parse().map(Unban::Peer).map_err(|_| ApiError::BadRequest(format!("{peer} is not a peer id"))),
        (None, None, Some(ip)) => ip.trim().parse().map(Unban::Ip).map_err(|_| ApiError::BadRequest(format!("{ip} is not an IP address"))),
        _ => Err(ApiError::BadRequest(r#"give one of {"all": true}, {"peer": "12D3…"} or {"ip": "203.0.113.7"}"#.to_string())),
    }
}

/// The transactions waiting in the node's mempool, by sender and nonce.
async fn mempool(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let limit = page(&api.tuned, &query, DEFAULT_MEMPOOL, MAX_MEMPOOL)? as usize;
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

async fn account(Shared(api): Shared<Api>, Path(address): Path<String>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let address = parse_address(&address)?;
    let (state, header) = state_at(&api, query.get("block").map(String::as_str))?;
    let account = Accounts::new(&state).get(&address)?;
    #[cfg_attr(not(feature = "names"), allow(unused_mut))]
    let mut answer = json::account(&address, &account);
    answer["height"] = header.height.into();

    // Its primary name, for a reverse lookup, while that name still resolves to it.
    #[cfg(feature = "names")]
    if let Some(names) = &api.rules.names {
        use crate::registry::names::Names;

        answer["name"] = Names::new(&state).primary(&address, header.timestamp, grace_ms(&state, names)?)?.map(|name| names.display(&name)).into();
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
    Ok(Changes::new(state).names()?.map_or_else(|| names.prices.clone(), |settings| settings.prices))
}

/// The grace names are judged by now: governance's last decided, else genesis's.
#[cfg(feature = "names")]
fn grace_ms(state: &State, names: &crate::registry::names::NamesConfig) -> Result<u64, ApiError> {
    Ok(Changes::new(state).names()?.map_or(names.grace_ms, |settings| settings.grace_ms))
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
    let grace = grace_ms(&state, names)?;
    let found = Names::new(&state).lookup(&name, now, grace)?;
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
        "grace_ends": found.as_ref().map(|found| found.expires.saturating_add(grace)),
        "texts": record.map_or_else(serde_json::Map::new, |record| record.texts.iter().map(|text| (text.key.clone(), Value::from(text.value.clone()))).collect()),
        "price_per_year": per_year,
    })))
}

/// Every name with a record, top-level and subnames, in name order, a page at a time from the one
/// after ?after: each with its status, owner, target and when it expires.
#[cfg(feature = "names")]
async fn names_list(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    use crate::registry::names::{NameStatus, Names, record_key};

    let names = names_config(&api)?;
    let limit = page(&api.tuned, &query, DEFAULT_PAGE, MAX_PAGE)?.max(1) as usize;
    let db = api.db.clone();
    let now = tip(&*db)?.header.timestamp;
    let prefix = record_key("");
    // Strictly after the name given: its own key, then the least byte past it.
    let from = query.get("after").map_or_else(|| prefix.clone(), |after| [record_key(&names.strip(after)), vec![0]].concat());
    let mut listed = Vec::new();
    let mut next = None;

    db.scan(Column::Names, &prefix, &from, &mut |key, _| {
        let name = String::from_utf8_lossy(&key[prefix.len()..]).into_owned();

        if listed.len() == limit {
            next = listed.last().cloned();

            return false;
        }

        listed.push(name);

        true
    })?;

    let state = State::new(db);
    let registry = Names::new(&state);
    let mut found = Vec::new();

    for name in &listed {
        let looked = registry.lookup(name, now, grace_ms(&state, names)?)?;
        let record = looked.as_ref().map(|looked| &looked.record);

        found.push(json!({
            "name": name,
            "display": names.display(name),
            "status": looked.as_ref().map_or(NameStatus::Available, |looked| looked.status).name(),
            "owner": record.map(|record| record.owner.to_string()),
            "target": record.and_then(|record| record.target).map(|target| target.to_string()),
            "expires": looked.as_ref().map(|looked| looked.expires),
        }));
    }

    Ok(Json(json!({ "names": found, "next": next })))
}

/// The names an account owns that still stand, in name order, a page at a time: each with its
/// status and when it expires.
#[cfg(feature = "names")]
async fn account_names(Shared(api): Shared<Api>, Path(address): Path<String>, Query(query): Query<HashMap<String, String>>) -> Answer {
    use crate::registry::names::{Names, stored_owned_names};

    let names = names_config(&api)?;
    let address = parse_address(&address)?;
    let limit = page(&api.tuned, &query, DEFAULT_ACCOUNT_TXS, MAX_ACCOUNT_TXS)?.max(1) as usize;
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
        if let Some(found) = registry.lookup(&name, now, grace_ms(&state, names)?)?
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

/// The multisigs an address's key is a member of, a page at a time, from the member index, for a
/// node that keeps one. The address may be given as the key itself, as a multisig names its
/// members. Each is read back from the registry, as it is now, so one whose members changed since
/// the index listed it is left out rather than shown wrong.
async fn account_multisigs(Shared(api): Shared<Api>, Path(address): Path<String>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;

    if !stored_member_index(&*db)? {
        return Err(ApiError::NotFound("this node keeps no member index; start it with --index-multisigs".to_string()));
    }

    // An Ethereum or Bitcoin account's key is not its address, which is a hash of it.
    let member = match address.parse::<crate::crypto::keypair::PublicKey>() {
        Ok(key) => key.address(),
        Err(_) => parse_address(&address)?,
    };
    let limit = page(&api.tuned, &query, DEFAULT_ACCOUNT_TXS, MAX_ACCOUNT_TXS)?.max(1) as usize;
    let after = query.get("cursor").map(|cursor| parse_address(cursor)).transpose()?;
    let (found, next) = stored_multisigs_of(&*db, &member, after.as_ref(), limit)?;
    let state = State::new(db);
    let registry = crate::registry::Multisigs::new(&state);
    let mut listed = Vec::new();

    for multisig in found {
        if let Some(policy) = registry.get(&multisig)?
            && policy.member_at(&member).is_some()
        {
            listed.push(json!({
                "address": multisig.to_string(),
                "threshold": policy.threshold,
                "members": policy.members.len(),
                "bitcoin": policy.members.iter().all(|member| member.bitcoin.is_some()),
                "evm": policy.members.iter().all(|member| member.evm.is_some()),
            }));
        }
    }

    Ok(Json(json!({ "member": member.to_string(), "multisigs": listed, "next": next.map(|next| next.to_string()) })))
}

/// The transactions touching an address, newest first, a page at a time: from the address index,
/// for a node that keeps one. Each with what it did, as a block lists it. Filters narrow it without
/// changing what the index holds: each entry is read and tested, no more of them per request than
/// MAX_TXS_SCANNED, so a page may come back short with a cursor that goes on from where it stopped.
async fn account_txs(Shared(api): Shared<Api>, Path(address): Path<String>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let db = api.db;
    let Some(index) = stored_index(&*db, IndexKind::Addresses)? else {
        return Err(ApiError::NotFound("this node keeps no address index; start it with --index-addresses".to_string()));
    };
    let address = parse_address(&address)?;
    let limit = page(&api.tuned, &query, DEFAULT_ACCOUNT_TXS, MAX_ACCOUNT_TXS)?.max(1) as usize;
    let cursor = query
        .get("cursor")
        .map(|cursor| from_hex(cursor).ok_or_else(|| ApiError::BadRequest("cursor is not hex".to_string())))
        .transpose()?;
    let filter = filters::TxFilter::parse(&query)?;
    let (lowest, highest) = filter.heights(&*db)?;
    // A cursor from an earlier page is further on than where the filter's newest height starts.
    let seek = highest.map(|height| address_txs_from(&address, height));
    let start = match (cursor, seek) {
        (Some(cursor), Some(seek)) => Some(cursor.max(seek)),
        (cursor, seek) => cursor.or(seek),
    };
    let mut txs = Vec::new();
    let scan = scan_address_txs(&*db, &address, start.as_deref(), lowest, limit, api.tuned.filter_scan.max(1), &mut |indexed| {
        let block = stored_block(&*db, &indexed.block)?;
        let receipt = stored_receipt(&*db, &indexed.tx)?;

        // A reorg between the reads took it off the main chain: the next page read has it right.
        if let (Some(block), Some(receipt)) = (block, receipt)
            && receipt.block == indexed.block
            && let Some(signed) = block.txs.get(indexed.index as usize)
            && filter.keeps(&address, signed, &receipt, block.header.timestamp)
        {
            let mut listed = json::indexed_tx(indexed, signed, &receipt, api.hash);
            listed["time"] = block.header.timestamp.into();
            txs.push(listed);

            return Ok(true);
        }

        Ok(false)
    })?;
    let tip = tip(&*db)?.header.height;

    Ok(Json(json!({
        "txs": txs,
        "next": scan.next.map(|next| to_hex(&next)),
        "indexed_from": index.from,
        "catching_up_to": (!index.caught_up(tip)).then(|| index.keep.lowest(tip)),
        "applied": filter.applied(),
        "scanned": scan.scanned,
        "limited": scan.limited,
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

/// What a walking light client wants next, in one answer: BUNDLE_EPOCHS' worth of headers from
/// ?headers= and of finality from ?epoch=, under the kinds of finality that vote; 404 otherwise, for
/// the client to ask for each as it goes. It checks them itself either way.
async fn light_bundle(Shared(api): Shared<Api>, Query(query): Query<HashMap<String, String>>) -> Answer {
    let interval = api
        .light
        .finality
        .and_then(|finality| finality.checkpoint_interval())
        .ok_or_else(|| ApiError::NotFound("this chain's finality does not vote".to_string()))?;
    let from = number(&query, "headers")?.unwrap_or(1);
    let epoch = number(&query, "epoch")?.unwrap_or(0);
    let count = crate::light::BUNDLE_EPOCHS.saturating_mul(interval).min(u64::from(MAX_LIGHT_HEADERS));
    let top = tip(&*api.db)?.header.height;
    let mut headers = Vec::new();

    for height in from..from.saturating_add(count) {
        match stored_header_at(&*api.db, height)? {
            Some(sealed) => headers.push(to_hex(&sealed.hashable_bytes_with_tag())),
            None => break,
        }
    }

    let steps = stored_light_finality(&*api.db, interval, epoch, crate::light::BUNDLE_EPOCHS, top)?;

    Ok(Json(json!({
        "tip": top,
        "headers": headers,
        "steps": steps.iter().map(|step| to_hex(&step.hashable_bytes_with_tag())).collect::<Vec<_>>(),
    })))
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
    let limit = page(&api.tuned, &query, DEFAULT_TOKENS, MAX_TOKENS)?.max(1) as usize;
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
    let limit = page(&api.tuned, &query, DEFAULT_TOKENS, MAX_TOKENS)?.max(1) as usize;
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
            | MempoolError::CannotPay
            | MempoolError::TooFarAhead { .. }
            | MempoolError::Multisig(_) => ApiError::BadRequest(err.to_string()),
            MempoolError::Stale | MempoolError::Duplicate | MempoolError::ReplacementTooLow { .. } => ApiError::Conflict(err.to_string()),
            MempoolError::TooMany { .. } => ApiError::TooMany(err.to_string()),
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
    let open = api.fees.as_ref().map_or(0, |fees| fees.offer_times(base, api.tx_fuel, &probe, OfferChoice::default(), api.tuned.suggest).gas_limit);
    // What runs code, or names, is tried first: what it would use, and whether it would fail.
    let ran = match &probe.tx {
        Tx::ContractCall { .. } | Tx::EvmCall { .. } | Tx::Ethereum { .. } | Tx::Deploy { .. } | Tx::Upgrade { .. } | Tx::Name { .. } | Tx::PrimaryName { .. } | Tx::Batch { .. } => Some(dry_run_off_loop(&api, probe.clone(), open, base).await?),
        // A multisig's batch is tried as its members will send it.
        Tx::Multisig { tx, .. } if matches!(**tx, Tx::Batch { .. }) => Some(dry_run_off_loop(&api, probe.clone(), open, base).await?),
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

            fees.offer_times(base, api.tx_fuel, &probe, chosen, api.tuned.suggest)
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
    let at = match body.get("block") {
        None | Some(Value::Null) => None,
        Some(Value::Number(height)) => Some(height.to_string()),
        Some(Value::String(block)) => Some(block.clone()),
        Some(_) => return Err(ApiError::BadRequest("block is a height or a 0x block id".to_string())),
    };
    let (vm, limits, function) = (api.vm.clone(), api.limits, Hash::Xxh3_64(selector(function)));
    let (at, header) = state_at(&api, at.as_deref())?;
    let block = BlockAt {
        height: header.height,
        time: header.timestamp,
        producer: header.producer,
        fuel_per_gas: api.fees.as_ref().map_or(1, |fees| fees.schedule().fuel_for(1).max(1)),
        vm: api.rules.vm,
    };
    // On a blocking thread, since the API answers everything on one.
    let outcome = tokio::task::spawn_blocking(move || {
        let state = Arc::new(State::new(at));
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
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError> {
    use tokio_stream::{StreamExt, wrappers::BroadcastStream, wrappers::errors::BroadcastStreamRecvError};

    let events = api
        .node
        .as_ref()
        .and_then(NodeHandle::subscribe)
        .ok_or_else(|| ApiError::NotFound("this node does not stream; start it with --stream".to_string()))?;
    let (sharing, _) = api.shared_with(peer, &headers);
    let (hash, reward) = (api.hash, api.reward);
    let events = BroadcastStream::new(events).filter_map(move |event| {
        let (name, data) = match event {
            // Peers kept to the node are not told of coming and going either.
            Ok(crate::node::NodeEvent::PeerUp { .. } | crate::node::NodeEvent::PeerDown(_)) if sharing.hides(sharing::ReportField::Peers) => return None,
            Ok(event) => json::node_event(&event, hash, &reward, sharing),
            Err(BroadcastStreamRecvError::Lagged(missed)) => ("lagged", json!({ "missed": missed })),
        };

        Some(Ok(Event::default().event(name).data(data.to_string())))
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
/// The state at the block a request names, and that block's header: the tip's as it is, for none
/// or latest; an older main-chain block's, by its height or 0x id, through history.
fn state_at(api: &Api, block: Option<&str>) -> Result<(Arc<State>, crate::structs::block::BlockHeader), ApiError> {
    let id = match block {
        None | Some("latest") => return Ok((Arc::new(State::new(api.db.clone())), tip(&*api.db)?.header)),
        Some(text) => match text.parse::<u64>() {
            Ok(height) => stored_header_at(&*api.db, height)?.ok_or_else(|| ApiError::NotFound(format!("no block at height {height}")))?.id(),
            Err(_) => block_id(text, api.hash)?,
        },
    };
    let state = stored_state_at(api.db.clone(), &id)?.ok_or_else(|| {
        ApiError::NotFound(format!("block {id} is not on the main chain, or older than the state this node keeps (--state-history)"))
    })?;
    let header = stored_header(&*api.db, &id)?.ok_or_else(|| ApiError::NotFound(format!("no block {id}")))?.header;

    Ok((state, header))
}

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
