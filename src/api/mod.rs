//! A read-only HTTP API over the committed chain, answering in JSON. It runs on a thread of its
//! own and reads the database directly, so a slow client never holds up the node loop.

use {
    crate::chain::{stored_block, stored_block_at, stored_tip},
    crate::db::{DbError, Read, State},
    crate::registry::{Accounts, Contracts},
    crate::structs::address::Address,
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    crate::structs::hex::from_hex,
    axum::extract::{Path, Query, State as Shared},
    axum::http::StatusCode,
    axum::response::{IntoResponse, Response},
    axum::routing::get,
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

type Db = Arc<dyn Read>;

#[document(
    summary = "Starts the HTTP API on a thread of its own. It answers GET requests in JSON from the committed chain and never writes.",
    category = "Node",
    params = {
        db = "The node's database. Start the API after Node::new, so genesis is in it.",
        address = "Where to listen.",
    },
    returns = "The address it listens on, which tells you the port when address asked for port 0.",
    note = "Routes: /status; /blocks, newest first, paged with ?before=height&limit=n (20 by default, at most 100); /blocks/{block}, where block is latest, a height or a 0x block id; /accounts/{address}; /contracts/{address}. Addresses are written as their kind and hex, such as ed25519:0x3b6a…. Amounts, and integers that can pass 2^53, are decimal strings; hashes are 0x hex. Errors are {\"error\": message} with status 400, 404 or 500.",
    see_also = "Node, Address",
)]
pub fn start(db: Arc<dyn Read>, address: SocketAddr) -> std::io::Result<SocketAddr> {
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

                if let Err(err) = axum::serve(listener, router(db)).await {
                    log::error!("[http] {err}");
                }
            })
        })?;

    Ok(local)
}

fn router(db: Db) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/blocks", get(blocks))
        .route("/blocks/{block}", get(block))
        .route("/accounts/{address}", get(account))
        .route("/contracts/{address}", get(contract))
        .fallback(async || ApiError::NotFound("no such route".to_string()))
        .with_state(db)
}

/// Why a request failed. Each becomes a status and {"error": message}.
enum ApiError {
    BadRequest(String),
    NotFound(String),
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
            ApiError::Db(err) => {
                log::error!("[http] {err}");

                (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
            }
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}

type Answer = Result<Json<Value>, ApiError>;

async fn status(Shared(db): Shared<Db>) -> Answer {
    let genesis = at_height(&*db, 0)?;
    let tip = tip(&*db)?;

    Ok(Json(json!({
        "genesis": genesis.id().to_string(),
        "tip": tip.id().to_string(),
        "height": tip.header.height,
        "timestamp": tip.header.timestamp,
    })))
}

/// Block summaries, newest first, below the height in ?before.
async fn blocks(Shared(db): Shared<Db>, Query(query): Query<HashMap<String, String>>) -> Answer {
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

async fn block(Shared(db): Shared<Db>, Path(block): Path<String>) -> Answer {
    let found = if block == "latest" {
        tip(&*db)?
    } else if block.starts_with("0x") {
        let id = block_id(&block)?;

        stored_block(&*db, &id)?.ok_or_else(|| ApiError::NotFound(format!("no block {id}")))?
    } else {
        let height = block
            .parse()
            .map_err(|_| ApiError::BadRequest(format!("not a height or a block id: {block}")))?;

        stored_block_at(&*db, height)?
            .ok_or_else(|| ApiError::NotFound(format!("no block at height {height}")))?
    };

    Ok(Json(json::block(&found)))
}

async fn account(Shared(db): Shared<Db>, Path(address): Path<String>) -> Answer {
    let address = parse_address(&address)?;
    let account = Accounts::new(&State::new(db)).get(&address)?;

    Ok(Json(json::account(&address, &account)))
}

async fn contract(Shared(db): Shared<Db>, Path(address): Path<String>) -> Answer {
    let address = parse_address(&address)?;
    let state = Arc::new(State::new(db));
    let record = Contracts::new(&state)
        .get(&address)?
        .ok_or_else(|| ApiError::NotFound(format!("no contract at {address}")))?;

    Ok(Json(json::contract(&address, &record)))
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

fn block_id(text: &str) -> Result<Hash, ApiError> {
    from_hex(text)
        .and_then(|bytes| bytes.try_into().ok())
        .map(Hash::Blake3_256)
        .ok_or_else(|| ApiError::BadRequest(format!("a block id is 0x and 64 hex digits: {text}")))
}

fn parse_address(text: &str) -> Result<Address, ApiError> {
    text.parse()
        .map_err(|err| ApiError::BadRequest(format!("{err}: {text}")))
}

#[cfg(test)]
mod tests;
