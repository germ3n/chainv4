//! Where workers mine for this node: GET /mining tells the apps its mining port, as they reach it,
//! and what it takes on it; GET /mining/ws carries the port's protocol over a WebSocket, for a
//! browser, which opens no TCP connections of its own.
//!
//! The WebSocket only relays: each text message is a line of the protocol, passed to the port on
//! this machine as it is and back, so the handshake proves the token to the port itself and the
//! MACs are the worker's and the port's alone. The port sees the relay as a connection from this
//! machine.

use {
    super::sharing::ReportField,
    super::{Answer, Api, ApiError, Shared, report},
    crate::mining::protocol::MOST_LINE,
    axum::Json,
    axum::extract::ConnectInfo,
    axum::extract::ws::{Message, WebSocket, WebSocketUpgrade},
    axum::http::HeaderMap,
    axum::response::{IntoResponse, Response},
    libp2p::futures::{SinkExt, StreamExt},
    serde_json::json,
    std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    tokio::io::{AsyncBufReadExt, AsyncWriteExt},
};

/// The route a browser mines over, as GET /mining names it.
pub const SOCKET_PATH: &str = "/mining/ws";

/// The mining port this node listens on, from its report: a 404 for a node that takes no workers.
async fn port(api: &Api) -> Result<(crate::consensus::MiningReport, SocketAddr), ApiError> {
    let mining = report(api, 0).await?.mining;
    let listening = mining.as_ref().and_then(|mining| mining.listening.as_deref().and_then(|listening| listening.parse().ok()));

    match (mining, listening) {
        (Some(mining), Some(listening)) => Ok((mining, listening)),
        _ => Err(ApiError::NotFound("this node takes no workers: it was not started with --mining".to_string())),
    }
}

/// Where the relay reaches the port: on this machine, whatever address it listens on.
fn local(listening: SocketAddr) -> SocketAddr {
    match listening.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listening.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listening.port()),
        _ => listening,
    }
}

/// GET /mining: the mining port, where the apps reach it, the protocol it speaks, and what it
/// takes: a pool's workers, workers mining alone, and anyone, or only those who know the token.
pub(super) async fn about(Shared(api): Shared<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Answer {
    let (sharing, _) = api.shared_with(peer, &headers);
    let (mining, listening) = port(&api).await?;
    let pool = mining.pool.as_ref();
    let counted = !sharing.hides(ReportField::Workers) && !sharing.hides(ReportField::Mining);

    Ok(Json(json!({
        "port": listening.port(),
        "public": api.mining_public.as_deref(),
        "onion": api.mining_onion.as_ref().and_then(|onion| onion.address()),
        "protocol": mining.protocol,
        "pool": pool.is_some(),
        "solo": mining.solo,
        "open": mining.open,
        "fee": pool.map(|pool| pool.fee),
        "window": pool.map(|pool| pool.window),
        "ws": SOCKET_PATH,
        "workers": counted.then_some(mining.workers.len()),
    })))
}

/// GET /mining/ws: the mining port over a WebSocket, each message a line of its protocol.
pub(super) async fn socket(Shared(api): Shared<Api>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Result<Response, ApiError> {
    let (_, listening) = port(&api).await?;
    let to = local(listening);

    // The relay outlives this request, so the per-request gate does not cover it: count it here,
    // by who asked, and let the count go when the relay ends.
    let who = super::asker(peer, &headers, &api.trusted_proxies);
    let Some(counted) = super::InFlight::enter(&api.mining_relays, who) else {
        return Err(ApiError::Unavailable("too many mining relays open from you at once; close one and try again".to_string()));
    };

    Ok(upgrade.max_message_size(MOST_LINE as usize).on_upgrade(move |socket| relay(socket, to, counted)).into_response())
}

/// Passes lines between the WebSocket and the mining port until either closes. Holds the relay's
/// count until it returns, which drops it.
async fn relay(socket: WebSocket, to: SocketAddr, _counted: super::Counted) {
    let stream = match tokio::net::TcpStream::connect(to).await {
        Ok(stream) => stream,
        Err(err) => return log::warn!("[http] the mining port {to} did not take the WebSocket's relay: {err}"),
    };
    let _ = stream.set_nodelay(true);
    let (read, mut write) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(read).lines();
    let (mut out, mut into) = socket.split();

    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    if out.send(Message::Text(line.into())).await.is_err() {
                        break;
                    }
                }
                _ => break,
            },
            message = into.next() => match message {
                // A line in a message, never more: one with its own line end would pass for two.
                Some(Ok(Message::Text(text))) if !text.contains('\n') => {
                    if write.write_all(format!("{}\n", text.as_str()).as_bytes()).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => break,
            },
        }
    }

    let _ = write.shutdown().await;
    let _ = out.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relay_reaches_a_port_listening_on_every_address_on_this_machine() {
        assert_eq!(local("0.0.0.0:4002".parse().unwrap()), "127.0.0.1:4002".parse().unwrap());
        assert_eq!(local("[::]:4002".parse().unwrap()), "[::1]:4002".parse().unwrap());
        assert_eq!(local("10.0.0.5:4002".parse().unwrap()), "10.0.0.5:4002".parse().unwrap());
    }
}
