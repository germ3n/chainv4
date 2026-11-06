//! The faucet, for dev and test chains: the node's --faucet-key pays a drip to whoever asks at POST
//! /faucet, once per address and once per IP each period, each request carrying a moment's proof of
//! work (crate::faucet). The coins are the operator's own, so any chain may run one; the docs say
//! what it is for. Who had a drip when is kept in the database's faucet column until the period
//! ends, so a restart lets no one ask again early.
//!
//! Behind a proxy on this machine or its private network, as the Docker stack's nginx is, the IP is
//! the last one X-Forwarded-For names: the one the proxy added itself, which a caller cannot forge.

use {
    super::{Answer, Api, ApiError, asker, at_height, json_body, parse_address, tip},
    crate::crypto::keypair::Keypair,
    crate::db::{Changes, Column, Database},
    crate::faucet::{FAUCET_WINDOW_MS, enough_work, faucet_hash, faucet_window},
    crate::mempool::MempoolError,
    crate::node::SubmitError,
    crate::registry::Accounts,
    crate::structs::tx::{FeeOffer, SignedTx, Tx},
    crate::structs::u256::u256,
    axum::Json,
    axum::body::Bytes,
    axum::extract::{ConnectInfo, State as Shared},
    axum::http::HeaderMap,
    r#macro::document,
    serde_json::{Value, json},
    std::collections::HashMap,
    std::net::SocketAddr,
    std::sync::{Arc, Mutex},
};

#[document(
    summary = "A node's faucet: the key that pays, how much each drip is, how often one address or IP may have one, and how much work a request carries.",
    category = "Node",
    see_also = "ApiOptions, faucet_hash",
)]
#[derive(Clone)]
pub struct FaucetConfig {
    #[document(summary = "The key whose account pays: fund it first, as any account.")]
    pub key: Keypair,
    #[document(summary = "Each drip, in the coin's smallest units.")]
    pub amount: u256,
    #[document(summary = "How long one address, and one IP, wait between drips, in milliseconds.")]
    pub every_ms: u64,
    #[document(summary = "How many zero bits a request's proof of work starts with: about 2^bits hashes for the asker. 0 asks for none.")]
    pub work_bits: u32,
}

/// Never the key, so a faucet's settings can be logged.
impl std::fmt::Debug for FaucetConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FaucetConfig")
            .field("address", &self.key.public().address())
            .field("amount", &self.amount)
            .field("every_ms", &self.every_ms)
            .field("work_bits", &self.work_bits)
            .finish()
    }
}

/// The faucet running: its settings, who had a drip when, and the nonce its next one takes.
pub(super) struct Faucet {
    config: FaucetConfig,
    /// When each address and IP last had a drip, in milliseconds, by "to:…" and "ip:…": the
    /// database's faucet column as it stands, read once at the start.
    given: Mutex<HashMap<String, u64>>,
    /// Where given is kept, so a restart forgets none of it.
    db: Arc<dyn Database>,
    /// The nonce after the last drip sent, which the chain may not have reached yet.
    next_nonce: Mutex<u64>,
}

impl Faucet {
    /// The faucet, with the drips kept in db whose period has not ended by now; those that have
    /// are let go.
    pub(super) fn new(config: FaucetConfig, db: Arc<dyn Database>, now: u64) -> Faucet {
        let mut given = HashMap::new();
        let mut ended = Changes::new();
        let read = db.scan(Column::Faucet, &[], &[], &mut |key, value| {
            match (std::str::from_utf8(key), <[u8; 8]>::try_from(value).map(u64::from_be_bytes)) {
                (Ok(name), Ok(at)) if now.saturating_sub(at) < config.every_ms => {
                    given.insert(name.to_string(), at);
                }
                _ => {
                    ended.insert((Column::Faucet, key.to_vec()), None);
                }
            }

            true
        });

        if let Err(err) = read.and_then(|()| if ended.is_empty() { Ok(()) } else { db.commit(ended) }) {
            log::warn!("[faucet] the drips kept could not be read, so this start remembers none: {err}");
        }

        Faucet {
            config,
            given: Mutex::new(given),
            db,
            next_nonce: Mutex::new(0),
        }
    }

    /// Writes these keys' times to the database, None letting a key go.
    fn keep(&self, keys: impl IntoIterator<Item = (String, Option<u64>)>) {
        let changes: Changes = keys.into_iter().map(|(key, at)| ((Column::Faucet, key.into_bytes()), at.map(|at| at.to_be_bytes().to_vec()))).collect();

        if !changes.is_empty()
            && let Err(err) = self.db.commit(changes)
        {
            log::warn!("[faucet] a drip could not be kept, so a restart forgets it: {err}");
        }
    }

    /// What clients need to ask: who pays, how much, how often, and how much work.
    pub(super) fn json(&self) -> Value {
        json!({
            "address": self.config.key.public().address().to_string(),
            "amount": self.config.amount.to_string(),
            "every_ms": self.config.every_ms,
            "work_bits": self.config.work_bits,
            "window_ms": FAUCET_WINDOW_MS,
        })
    }
}

/// GET /faucet: the faucet's settings, and how much it holds.
pub(super) async fn about(Shared(api): Shared<Api>) -> Answer {
    let faucet = api.faucet.clone().ok_or_else(|| ApiError::NotFound("this node runs no faucet".to_string()))?;
    let mut about = faucet.json();
    let holds = Accounts::new(&crate::db::State::new(api.db.clone())).get(&faucet.config.key.public().address())?.balance;
    about["balance"] = holds.to_string().into();

    Ok(Json(about))
}

/// POST /faucet with {"to": "ed25519:0x…", "window": n, "nonce": n}: a drip to that address, as a
/// transfer from the faucet's key, once the work holds and neither the address nor the IP had one
/// this period.
pub(super) async fn give(Shared(api): Shared<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Answer {
    let faucet = api.faucet.clone().ok_or_else(|| ApiError::NotFound("this node runs no faucet".to_string()))?;
    let node = api.node.clone().ok_or_else(|| ApiError::Unavailable("this API has no node to send the drip".to_string()))?;
    let body = json_body(&body)?;
    let to = parse_address(body["to"].as_str().ok_or_else(|| ApiError::BadRequest("to is the address to pay".to_string()))?)?;
    let number = |name: &str| -> Result<u64, ApiError> {
        match &body[name] {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse().ok(),
            _ => None,
        }
        .ok_or_else(|| ApiError::BadRequest(format!("{name} is a whole number")))
    };
    let (window, nonce) = (number("window")?, number("nonce")?);
    let now = now_ms();
    let current = faucet_window(now);

    // This window's, or the last one's, worked out on the chain this node follows.
    if window != current && window + 1 != current {
        return Err(ApiError::BadRequest("the work is for another time window: work it out again".to_string()));
    }

    let genesis = at_height(&*api.db, 0)?.id();

    if !enough_work(&faucet_hash(&genesis, &to, window, nonce), faucet.config.work_bits) {
        return Err(ApiError::BadRequest(format!("not enough work: the hash must start with {} zero bits", faucet.config.work_bits)));
    }

    let ip = asker(peer, &headers, &api.trusted_proxies);
    let keys = [format!("to:{to}"), format!("ip:{ip}")];
    reserve(&faucet, &keys, now)?;

    match send(&api, &faucet, node, to).await {
        Ok(id) => {
            log::info!("[faucet] {} to {to}, {id}", faucet.config.amount);

            Ok(Json(json!({ "id": id, "to": to.to_string(), "amount": faucet.config.amount.to_string() })))
        }
        Err(err) => {
            // Nothing went out: the address and IP may ask again.
            let mut given = faucet.given.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

            for key in &keys {
                given.remove(key);
            }

            faucet.keep(keys.iter().map(|key| (key.clone(), None)));

            Err(err)
        }
    }
}

/// Holds the address and the IP to this drip, unless either had one this period.
fn reserve(faucet: &Faucet, keys: &[String], now: u64) -> Result<(), ApiError> {
    let every = faucet.config.every_ms;
    let mut given = faucet.given.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut kept = Vec::new();
    given.retain(|key, at| {
        let live = now.saturating_sub(*at) < every;

        if !live {
            kept.push((key.clone(), None));
        }

        live
    });

    if let Some(at) = keys.iter().filter_map(|key| given.get(key)).max() {
        let left = every.saturating_sub(now.saturating_sub(*at));
        faucet.keep(kept);

        return Err(ApiError::TooMany(format!("this address or this connection had a drip already: again in {}", span(left))));
    }

    for key in keys {
        given.insert(key.clone(), now);
        kept.push((key.clone(), Some(now)));
    }

    faucet.keep(kept);

    Ok(())
}

/// Signs the drip and hands it to the node: priced as POST /offer prices a transfer, at the nonce
/// after the faucet's last one, which a drip still waiting has not reached the chain with yet.
async fn send(api: &Api, faucet: &Arc<Faucet>, node: crate::node::NodeHandle, to: crate::structs::address::Address) -> Result<String, ApiError> {
    let key = &faucet.config.key;
    let from = key.public().address();
    let tx = Tx::Transfer { from, to, amount: crate::cli::amount(faucet.config.amount) };
    let genesis = at_height(&*api.db, 0)?.id();
    let header = tip(&*api.db)?.header;
    let base = api.fees.as_ref().map_or(u256::ZERO, |fees| fees.pricing().next_price(header.base_price, header.gas_used, fees.block_gas_limit()));

    // A nonce taken meanwhile, by a drip from another request, or the chain moving on: the next.
    for _ in 0..3 {
        let account = Accounts::new(&crate::db::State::new(api.db.clone())).get(&from)?;
        let nonce = {
            let next = faucet.next_nonce.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            account.nonce.max(*next)
        };
        let probe = SignedTx::sign(tx.clone(), nonce, FeeOffer::NONE, key, &genesis);
        let offer = api.fees.as_ref().map_or(FeeOffer::NONE, |fees| fees.offer_times(base, api.tx_fuel, &probe, Default::default(), api.tuned.suggest));
        let most = faucet.config.amount.saturating_add(u256::from(offer.gas_limit).saturating_mul(offer.max_price));

        if account.balance < most {
            return Err(ApiError::Unavailable(format!("the faucet is dry: it holds {} of the {most} a drip takes", account.balance)));
        }

        let signed = SignedTx::sign(tx.clone(), nonce, offer, key, &genesis);

        match node.submit(signed).await {
            Ok(id) => {
                *faucet.next_nonce.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = nonce + 1;

                return Ok(id.to_string());
            }
            Err(SubmitError::Refused(MempoolError::Stale | MempoolError::Duplicate)) => {
                *faucet.next_nonce.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = nonce + 1;
            }
            Err(SubmitError::Refused(MempoolError::Db(err))) => return Err(ApiError::Db(err)),
            // An address the chain takes no transfer to, such as bytes that are no key.
            Err(SubmitError::Refused(MempoolError::Invalid(why))) => return Err(ApiError::BadRequest(format!("the chain takes no drip to that address: {why}"))),
            Err(SubmitError::Refused(err)) => return Err(ApiError::Unavailable(format!("the node would not take the drip: {err}"))),
            Err(err) => return Err(ApiError::Unavailable(err.to_string())),
        }
    }

    Err(ApiError::Unavailable("the faucet's nonce kept being taken: ask again".to_string()))
}


/// A wait as people say it: 23h 59m, 5m, 30s.
fn span(ms: u64) -> String {
    let (hours, minutes, seconds) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1_000 % 60);

    match (hours, minutes) {
        (0, 0) => format!("{}s", seconds.max(1)),
        (0, minutes) => format!("{minutes}m"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_loopback_or_named_proxy_may_say_who_asks() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "6.6.6.6, 203.0.113.9".parse().unwrap());
        let loopback: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let proxy: SocketAddr = "172.18.0.3:40000".parse().unwrap();
        let other: SocketAddr = "172.18.0.4:40000".parse().unwrap();
        let outside: SocketAddr = "198.51.100.7:40000".parse().unwrap();
        let added = "203.0.113.9".parse::<std::net::IpAddr>().unwrap();
        let trusted = super::super::proxies::TrustedProxies::new(vec![proxy.ip().to_string()]);
        let none = super::super::proxies::TrustedProxies::default();

        // Loopback is always believed, as the proxy on the same host connects from it.
        assert_eq!(asker(loopback, &headers, &none), added, "the one the proxy added, not the one the caller wrote");
        // A private address is believed only when --trusted-proxy named it.
        assert_eq!(asker(proxy, &headers, &trusted), added, "a named proxy is believed");
        assert_eq!(asker(other, &headers, &trusted), other.ip(), "another box on the private network is not, so it cannot spoof");
        assert_eq!(asker(proxy, &headers, &none), proxy.ip(), "with none named, even a private peer is not believed");
        assert_eq!(asker(outside, &headers, &trusted), outside.ip(), "a caller from outside cannot name another IP");
        assert_eq!(asker(loopback, &HeaderMap::new(), &none), loopback.ip());
        assert_eq!((span(86_340_000), span(300_000), span(400)), ("23h 59m".to_string(), "5m".to_string(), "1s".to_string()));
    }

    #[test]
    fn a_restart_remembers_who_had_a_drip_until_their_period_ends() {
        let db: Arc<dyn Database> = Arc::new(crate::db::MemoryDb::default());
        let config = FaucetConfig { key: Keypair::development(), amount: u256::from(5u64), every_ms: 60_000, work_bits: 0 };
        let first = Faucet::new(config.clone(), db.clone(), 1_000);
        let alice = ["to:alice".to_string(), "ip:1.2.3.4".to_string()];
        let bob = ["to:bob".to_string(), "ip:5.6.7.8".to_string()];

        assert!(reserve(&first, &alice, 1_000).is_ok());
        assert!(reserve(&first, &bob, 2_000).is_ok());
        // Bob's drip did not go out, as give() lets it go.
        first.keep(bob.iter().map(|key| (key.clone(), None)));
        drop(first);

        let again = Faucet::new(config.clone(), db.clone(), 30_000);
        assert!(matches!(reserve(&again, &alice, 30_000), Err(ApiError::TooMany(_))), "the restart remembers alice");
        assert!(matches!(reserve(&again, &["to:carol".to_string(), "ip:1.2.3.4".to_string()], 30_000), Err(ApiError::TooMany(_))), "and her IP");
        assert!(reserve(&again, &bob, 30_000).is_ok());
        drop(again);

        // Past alice's period, a start lets her go, from the database too.
        let later = Faucet::new(config, db.clone(), 61_000);
        assert!(reserve(&later, &alice, 61_000).is_ok());
        let mut kept = Vec::new();
        db.scan(Column::Faucet, &[], &[], &mut |key, _| {
            kept.push(String::from_utf8(key.to_vec()).unwrap());
            true
        })
        .unwrap();
        assert_eq!(kept, ["ip:1.2.3.4", "ip:5.6.7.8", "to:alice", "to:bob"]);
    }
}
