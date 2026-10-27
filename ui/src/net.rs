//! The apps' requests to a node, through a proxy when one is set and never around it. On the
//! desktop and on Android every request goes out through one ureq agent built with the proxy, so
//! while a proxy is set there is no way out but through it: with the proxy down, requests fail
//! rather than go direct. SOCKS5 goes as SOCKS5h, so names are resolved by the proxy too, which is
//! what reaches a .onion node through Tor and keeps lookups off the local network. In the browser
//! the browser does the networking, and its own proxy settings apply.

use std::ops::ControlFlow;

/// A proxy every request goes through: SOCKS5 with the proxy resolving names, or HTTP CONNECT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy(String);

impl Proxy {
    /// Orbot's SOCKS port on Android, and Tor's own.
    pub const ORBOT: &'static str = "socks5h://127.0.0.1:9050";
    /// i2pd's SOCKS port, which resolves .i2p names inside I2P as Tor does .onion ones. The Java
    /// I2P router has an HTTP proxy at http://127.0.0.1:4444 instead, unless a SOCKS tunnel is
    /// made.
    pub const I2P: &'static str = "socks5h://127.0.0.1:4447";

    /// Reads socks5h://host:port, socks5://host:port, which is taken as socks5h so no name is
    /// looked up outside the proxy, or http://host:port; user:password@ may come before the host.
    pub fn parse(text: &str) -> Result<Proxy, String> {
        let text = text.trim();
        let (scheme, rest) = text
            .split_once("://")
            .ok_or_else(|| "a proxy is socks5h://host:port or http://host:port".to_string())?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "socks5h" | "socks5" => "socks5h",
            "http" => "http",
            other => return Err(format!("{other} proxies are not supported: use socks5h:// or http://")),
        };
        let rest = rest.trim_end_matches('/');
        let (host, port) = rest
            .rsplit_once(':')
            .ok_or_else(|| "give the proxy's port too, such as 127.0.0.1:9050".to_string())?;
        let at = host.rsplit_once('@').map_or(host, |(_, host)| host);

        if at.is_empty() || rest.contains(['/', ' ']) {
            return Err(format!("not a proxy address: {text}"));
        }

        port.parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| format!("not a port: {port}"))?;

        Ok(Proxy(format!("{scheme}://{rest}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Where it is, without any credentials, for messages.
    pub fn shown(&self) -> &str {
        let rest = self.0.split_once("://").map_or(self.0.as_str(), |(_, rest)| rest);

        rest.rsplit_once('@').map_or(rest, |(_, host)| host)
    }
}

/// A proxy setting as typed, as the apps keep it once a client builds with it: written the one
/// way, socks5 as socks5h, or empty to go direct.
pub fn setting(text: &str) -> Result<String, String> {
    Client::for_setting(text).map(|client| client.proxy().map_or_else(String::new, |proxy| proxy.as_str().to_string()))
}

/// An answer: its status and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// Whether the status is 2xx.
    pub ok: bool,
    pub status: u16,
    pub status_text: String,
    pub bytes: Vec<u8>,
}

/// A part of a stream: its head first, then its body as it comes, an empty chunk when it ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Head { ok: bool, status: u16, status_text: String },
    Chunk(Vec<u8>),
}

/// How an app talks to its node, with the proxy it was made with. Cheap to clone; clones share
/// connections.
#[derive(Clone)]
pub struct Client {
    proxy: Option<Proxy>,
    #[cfg(not(target_arch = "wasm32"))]
    agent: ureq::Agent,
    /// Without a time limit, for streams that stay open.
    #[cfg(not(target_arch = "wasm32"))]
    lasting: ureq::Agent,
}

impl Client {
    /// A client that sends everything through this proxy, or goes direct without one.
    pub fn new(proxy: Option<Proxy>) -> Result<Client, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let agent = native::agent(proxy.as_ref(), Some(std::time::Duration::from_secs(30)))?;
            let lasting = native::agent(proxy.as_ref(), None)?;

            Ok(Client { proxy, agent, lasting })
        }

        #[cfg(target_arch = "wasm32")]
        {
            Ok(Client { proxy })
        }
    }

    /// A client that goes direct.
    pub fn direct() -> Client {
        Client::new(None).expect("a direct client always builds")
    }

    /// A client for a proxy setting as the apps keep it: a proxy, or empty to go direct.
    pub fn for_setting(setting: &str) -> Result<Client, String> {
        match setting.trim() {
            "" => Ok(Client::direct()),
            text => Client::new(Some(Proxy::parse(text)?)),
        }
    }

    pub fn proxy(&self) -> Option<&Proxy> {
        self.proxy.as_ref()
    }

    /// Sends a GET, or a POST of JSON when there is a body, and calls done with the answer from
    /// another thread, or from the browser's event loop.
    pub fn fetch(&self, url: String, body: Option<Vec<u8>>, done: impl FnOnce(Result<Answer, String>) + Send + 'static) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (agent, proxy) = (self.agent.clone(), self.proxy.clone());

            std::thread::spawn(move || done(native::fetch(&agent, proxy.as_ref(), &url, body)));
        }

        #[cfg(target_arch = "wasm32")]
        web::fetch(url, body, done);
    }

    /// Opens a GET whose body keeps coming, such as an event stream, and calls on with each
    /// piece until it ends, fails or on breaks.
    pub fn stream(&self, url: String, on: impl FnMut(Result<Piece, String>) -> ControlFlow<()> + Send + 'static) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (agent, proxy) = (self.lasting.clone(), self.proxy.clone());

            std::thread::spawn(move || native::stream(&agent, proxy.as_ref(), &url, on));
        }

        #[cfg(target_arch = "wasm32")]
        web::stream(url, on);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use {
        super::{Answer, Piece, Proxy},
        std::io::Read,
        std::ops::ControlFlow,
        std::time::Duration,
    };

    pub fn agent(proxy: Option<&Proxy>, timeout: Option<Duration>) -> Result<ureq::Agent, String> {
        let mut config = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(timeout);

        // Set, it is the only way out, NO_PROXY or not; unset, ureq's own default, the
        // environment's ALL_PROXY, HTTPS_PROXY or HTTP_PROXY, applies.
        if let Some(proxy) = proxy {
            let proxy = ureq::Proxy::new(proxy.as_str()).map_err(|err| format!("the proxy {}: {err}", proxy.shown()))?;
            config = config.proxy(Some(proxy));
        }

        Ok(config.build().into())
    }

    /// Says a request failed, and through which proxy, since a stopped proxy is the usual reason.
    fn failed(proxy: Option<&Proxy>, err: ureq::Error) -> String {
        match proxy {
            Some(proxy) => format!("through the proxy at {}: {err}", proxy.shown()),
            None => err.to_string(),
        }
    }

    pub fn fetch(agent: &ureq::Agent, proxy: Option<&Proxy>, url: &str, body: Option<Vec<u8>>) -> Result<Answer, String> {
        let sent = match body {
            Some(body) => agent.post(url).header("content-type", "application/json").send(&body[..]),
            None => agent.get(url).call(),
        };
        let mut response = sent.map_err(|err| failed(proxy, err))?;
        let status = response.status();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(256 << 20)
            .read_to_vec()
            .map_err(|err| failed(proxy, err))?;

        Ok(Answer {
            ok: status.is_success(),
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or_default().to_string(),
            bytes,
        })
    }

    pub fn stream(agent: &ureq::Agent, proxy: Option<&Proxy>, url: &str, mut on: impl FnMut(Result<Piece, String>) -> ControlFlow<()>) {
        let response = match agent.get(url).call() {
            Ok(response) => response,
            Err(err) => {
                let _ = on(Err(failed(proxy, err)));

                return;
            }
        };
        let status = response.status();
        let head = Piece::Head {
            ok: status.is_success(),
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or_default().to_string(),
        };

        if on(Ok(head)).is_break() {
            return;
        }

        let mut reader = response.into_body().into_reader();
        let mut buffer = vec![0; 16 * 1024];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = on(Ok(Piece::Chunk(Vec::new())));

                    return;
                }
                Ok(read) => {
                    if on(Ok(Piece::Chunk(buffer[..read].to_vec()))).is_break() {
                        return;
                    }
                }
                Err(err) => {
                    let _ = on(Err(err.to_string()));

                    return;
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use {
        super::{Answer, Piece},
        std::ops::ControlFlow,
    };

    pub fn fetch(url: String, body: Option<Vec<u8>>, done: impl FnOnce(Result<Answer, String>) + Send + 'static) {
        // ehttp's own text/plain, which needs no preflight from a page on another origin.
        let request = match body {
            Some(body) => ehttp::Request::post(url, body),
            None => ehttp::Request::get(url),
        };

        ehttp::fetch(request, move |response| {
            done(response.map(|response| Answer {
                ok: response.ok,
                status: response.status,
                status_text: response.status_text,
                bytes: response.bytes,
            }))
        });
    }

    pub fn stream(url: String, on: impl FnMut(Result<Piece, String>) -> ControlFlow<()> + Send + 'static) {
        // ehttp calls back through a shared reference; the browser calls one piece at a time.
        let on = std::sync::Mutex::new(on);

        ehttp::streaming::fetch(ehttp::Request::get(url), move |part| {
            let mut on = on.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

            on(part.map(|part| match part {
                ehttp::streaming::Part::Response(head) => Piece::Head { ok: head.ok, status: head.status, status_text: head.status_text },
                ehttp::streaming::Part::Chunk(bytes) => Piece::Chunk(bytes),
            }))
        });
    }
}

#[cfg(test)]
mod tests;
