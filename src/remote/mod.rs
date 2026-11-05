//! Connections to other chains' services: Esplora and bitcoind for Bitcoin, an EVM chain's
//! JSON-RPC, and an Ethereum beacon API (docs/bridges.md, Endpoints). Each is a list of URLs that
//! stand in for each other ([`Endpoints`]): the first is asked; an error, a timeout or a 429 moves on
//! to the next; the one that answered stays in use, and the first is tried again a few minutes
//! later. None is trusted more for being first: what they serve is checked, by proofs on chain, by
//! the light clients, or by the transaction it is.
//!
//! Each URL may carry a login ([`Auth`]), from a file, the environment or bitcoind's cookie, never
//! from a flag's own text; and every connection may go through a SOCKS5 proxy. What is logged or
//! shown of a URL is its scheme, host and port alone ([`host_of`]): no userinfo, no path or query,
//! which may carry an API key.

pub mod beacon;
#[cfg(feature = "bitcoin")]
pub mod bitcoin;

use {
    r#macro::document,
    std::{
        path::PathBuf,
        sync::Mutex,
        time::{Duration, Instant},
    },
};

#[cfg(all(test, feature = "bitcoin"))]
mod tests;

#[document(
    summary = "How long a list of endpoints stays on a later one before it asks the first again: five minutes.",
    category = "Client",
    see_also = "Endpoints",
)]
pub const BACK_TO_FIRST: Duration = Duration::from_secs(300);

#[document(
    summary = "A request to an outside service.",
    category = "Client",
    see_also = "Http",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    #[document(summary = "GET or POST.")]
    pub post: bool,
    #[document(summary = "The whole URL.")]
    pub url: String,
    #[document(summary = "Headers, its login's among them.")]
    pub headers: Vec<(String, String)>,
    #[document(summary = "What a POST sends.")]
    pub body: Vec<u8>,
}

#[document(
    summary = "What an outside service answered: its status and its body.",
    category = "Client",
    see_also = "Http",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    #[document(summary = "The HTTP status.")]
    pub status: u16,
    #[document(summary = "The body's bytes.")]
    pub body: Vec<u8>,
}

impl Reply {
    #[document(parent = "Reply", summary = "Its body as text, lossily.")]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[document(
    summary = "What sends requests to outside services: ureq in the tools, a fake in tests. An error is a transport's own, a refused connection or a timeout, already written without the URL.",
    category = "Client",
    see_also = "UreqHttp, Endpoints",
)]
pub trait Http: Send + Sync {
    #[document(parent = "Http", summary = "Sends a request and reads the whole answer.")]
    fn send(&self, request: &Request) -> Result<Reply, String>;
}

#[document(
    summary = "Http over ureq, through a SOCKS5 proxy when given one: what the CLI and the node send outside requests with.",
    category = "Client",
    see_also = "Http, parse_proxy",
)]
pub struct UreqHttp {
    agent: ureq::Agent,
}

#[document(
    summary = "Checks a --proxy: socks5:// or socks5h://, the latter resolving names at the proxy, as Tor wants.",
    returns = "The proxy, or why it is none. The error names no userinfo.",
    category = "Client",
    see_also = "UreqHttp",
)]
pub fn parse_proxy(text: &str) -> Result<ureq::Proxy, String> {
    let scheme = text.split_once("://").map(|(scheme, _)| scheme.to_ascii_lowercase());

    if !matches!(scheme.as_deref(), Some("socks5" | "socks5h")) {
        return Err(format!("--proxy {} is not socks5://host:port or socks5h://host:port", host_of(text)));
    }

    ureq::Proxy::new(text).map_err(|_| format!("--proxy {} does not read", host_of(text)))
}

impl UreqHttp {
    #[document(parent = "UreqHttp", summary = "A client, through a proxy or straight, timing out after 30 seconds, connecting after 10.", returns = "The client, or why the proxy is none.")]
    pub fn new(proxy: Option<&str>) -> Result<UreqHttp, String> {
        let proxy = proxy.map(parse_proxy).transpose()?;
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .timeout_connect(Some(Duration::from_secs(10)))
            .proxy(proxy)
            .build()
            .into();

        Ok(UreqHttp { agent })
    }

    #[document(parent = "UreqHttp", summary = "One over an agent made elsewhere, such as the node API's client.")]
    pub fn from_agent(agent: ureq::Agent) -> UreqHttp {
        UreqHttp { agent }
    }
}

/// A transport error in words, without the URL or proxy it was for.
fn describe(err: &ureq::Error) -> String {
    match err {
        ureq::Error::Timeout(_) => "timed out".to_string(),
        ureq::Error::HostNotFound => "its host was not found".to_string(),
        ureq::Error::ConnectionFailed => "the connection failed".to_string(),
        ureq::Error::Io(err) => format!("the connection failed: {}", err.kind()),
        ureq::Error::BadUri(_) => "a URL that is not one".to_string(),
        ureq::Error::InvalidProxyUrl | ureq::Error::ConnectProxyFailed(_) => "the proxy failed".to_string(),
        ureq::Error::Tls(why) => format!("TLS failed: {why}"),
        ureq::Error::BodyExceedsLimit(limit) => format!("it answered more than {limit} bytes"),
        _ => "the request failed".to_string(),
    }
}

impl Http for UreqHttp {
    fn send(&self, request: &Request) -> Result<Reply, String> {
        let mut answer = match request.post {
            true => {
                let mut builder = self.agent.post(&request.url);

                for (name, value) in &request.headers {
                    builder = builder.header(name, value);
                }

                builder.send(&request.body[..])
            }
            false => {
                let mut builder = self.agent.get(&request.url);

                for (name, value) in &request.headers {
                    builder = builder.header(name, value);
                }

                builder.call()
            }
        }
        .map_err(|err| describe(&err))?;
        let status = answer.status().as_u16();
        let body = answer.body_mut().with_config().limit(64 << 20).read_to_vec().map_err(|err| describe(&err))?;

        Ok(Reply { status, body })
    }
}

#[document(
    summary = "A URL as it may be shown: its scheme, host and port, without any userinfo, path or query, which can carry keys.",
    category = "Client",
    see_also = "Endpoint",
)]
pub fn host_of(url: &str) -> String {
    let (scheme, rest) = url.trim().split_once("://").unwrap_or(("", url.trim()));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);

    match scheme {
        "" => host.to_string(),
        scheme => format!("{scheme}://{host}"),
    }
}

#[document(
    summary = "A login, read from where it was named, never from a flag's own text: bitcoind's cookie file, read again each time so a restarted bitcoind's new cookie is used; a file holding user:password or a token, refused while its group or others may read it; or an environment variable. user:password is sent as basic auth, anything without a colon as a bearer token.",
    category = "Client",
    see_also = "Endpoint, Auth::parse",
)]
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    #[document(summary = "None.")]
    None,
    #[document(summary = "A user and password, sent as basic auth.")]
    Basic { user: String, password: String },
    #[document(summary = "A token, sent as Authorization: Bearer.")]
    Bearer(String),
    #[document(summary = "bitcoind's cookie file, read at each request.")]
    Cookie(PathBuf),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::None => write!(f, "none"),
            Auth::Basic { .. } => write!(f, "a password"),
            Auth::Bearer(_) => write!(f, "a token"),
            Auth::Cookie(path) => write!(f, "the cookie at {}", path.display()),
        }
    }
}

/// Bytes as standard base64, padded: what basic auth sends.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let word = u32::from(chunk[0]) << 16 | u32::from(*chunk.get(1).unwrap_or(&0)) << 8 | u32::from(*chunk.get(2).unwrap_or(&0));

        for place in 0..4 {
            match place <= chunk.len() {
                true => out.push(ALPHABET[(word >> (18 - 6 * place) & 63) as usize] as char),
                false => out.push('='),
            }
        }
    }

    out
}

/// A secret's text as a login: user:password, or a token.
fn secret(text: &str) -> Result<Auth, String> {
    let text = text.trim();

    match text.split_once(':') {
        _ if text.is_empty() => Err("an empty login".to_string()),
        Some((user, password)) => Ok(Auth::Basic { user: user.to_string(), password: password.to_string() }),
        None => Ok(Auth::Bearer(text.to_string())),
    }
}

impl Auth {
    #[document(
        parent = "Auth",
        summary = "A login as an --…-auth flag names it: cookie:PATH, file:PATH or env:NAME. A cookie is bitcoind's alone.",
        params = { text = "The flag's value.", cookie = "Whether a cookie may be named: only for bitcoind." },
        returns = "The login, or why it is none. The error never holds the secret.",
    )]
    pub fn parse(text: &str, cookie: bool) -> Result<Auth, String> {
        match text.split_once(':') {
            Some(("cookie", path)) if cookie => Ok(Auth::Cookie(PathBuf::from(path))),
            Some(("cookie", _)) => Err("a cookie is bitcoind's: give file:PATH or env:NAME".to_string()),
            Some(("file", path)) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;

                    let mode = std::fs::metadata(path).map_err(|err| format!("{path}: {err}"))?.permissions().mode();

                    if mode & 0o044 != 0 {
                        return Err(format!("{path} may be read by its group or others: chmod 600 it"));
                    }
                }

                secret(&std::fs::read_to_string(path).map_err(|err| format!("{path}: {err}"))?).map_err(|why| format!("{path}: {why}"))
            }
            Some(("env", name)) => secret(&std::env::var(name).map_err(|_| format!("${name} is not set"))?).map_err(|why| format!("${name}: {why}")),
            _ => Err("a login is cookie:PATH, file:PATH or env:NAME, never the password itself".to_string()),
        }
    }

    #[document(parent = "Auth", summary = "The Authorization header it sends, if any: a cookie read now.")]
    pub fn header(&self) -> Result<Option<String>, String> {
        let basic = |user: &str, password: &str| format!("Basic {}", base64(format!("{user}:{password}").as_bytes()));

        match self {
            Auth::None => Ok(None),
            Auth::Basic { user, password } => Ok(Some(basic(user, password))),
            Auth::Bearer(token) => Ok(Some(format!("Bearer {token}"))),
            Auth::Cookie(path) => {
                let cookie = std::fs::read_to_string(path).map_err(|err| format!("bitcoind's cookie {}: {err}", path.display()))?;
                let (user, password) = cookie.trim().split_once(':').ok_or_else(|| format!("bitcoind's cookie {} does not read", path.display()))?;

                Ok(Some(basic(user, password)))
            }
        }
    }
}

#[document(
    summary = "One URL of a service, with its login.",
    category = "Client",
    see_also = "Endpoints, Auth",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    #[document(summary = "The URL, as given.")]
    pub url: String,
    #[document(summary = "Its login.")]
    pub auth: Auth,
}

impl Endpoint {
    #[document(parent = "Endpoint", summary = "Its URL as it may be shown.")]
    pub fn host(&self) -> String {
        host_of(&self.url)
    }

    #[document(parent = "Endpoint", summary = "A request to it: its URL with a path after it, its login in the headers.")]
    pub fn request(&self, post: bool, path: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Result<Request, EndpointFailure> {
        let mut all: Vec<(String, String)> = headers.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();

        if let Some(login) = self.auth.header().map_err(EndpointFailure::Endpoint)? {
            all.push(("authorization".to_string(), login));
        }

        Ok(Request { post, url: format!("{}{path}", self.url.trim().trim_end_matches('/')), headers: all, body })
    }
}

#[document(
    summary = "Why asking one endpoint failed: the endpoint failed, out of reach, timing out, throttling or refusing its login, and the next is asked; or it answered and the answer is the failure, which another would give too.",
    category = "Client",
    see_also = "Endpoints::ask",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndpointFailure {
    #[document(summary = "The endpoint failed: ask the next.")]
    Endpoint(String),
    #[document(summary = "The endpoint answered with a failure: stop there.")]
    Answer(String),
}

#[document(
    summary = "How a reply counts: a 429 or a 5xx, or a refused login, is the endpoint failing; anything else is its answer.",
    category = "Client",
    see_also = "EndpointFailure",
)]
pub fn judged(reply: Reply) -> Result<Reply, EndpointFailure> {
    match reply.status {
        429 => Err(EndpointFailure::Endpoint("it answered 429, too many requests".to_string())),
        401 | 403 => Err(EndpointFailure::Endpoint(format!("it refused the login, {}", reply.status))),
        500..=599 => Err(EndpointFailure::Endpoint(format!("it answered {}", reply.status))),
        _ => Ok(reply),
    }
}

/// Which endpoint is in use, and since when it has been one other than the first.
struct Current {
    index: usize,
    since: Option<Instant>,
}

#[document(
    summary = "URLs of one service that stand in for each other: the first asked, the next on an error, a timeout or a 429, the one that answered kept, and the first tried again after BACK_TO_FIRST. None is trusted more for being first.",
    category = "Client",
    see_also = "Endpoint, EndpointFailure, BACK_TO_FIRST",
)]
pub struct Endpoints {
    list: Vec<Endpoint>,
    current: Mutex<Current>,
    back_after: Duration,
}

impl Endpoints {
    #[document(parent = "Endpoints", summary = "A list, the first in use.")]
    pub fn new(list: Vec<Endpoint>) -> Endpoints {
        Endpoints { list, current: Mutex::new(Current { index: 0, since: None }), back_after: BACK_TO_FIRST }
    }

    #[document(
        parent = "Endpoints",
        summary = "A list from a flag's URLs and its logins, matched by order: the first login the first URL's, and URLs past the logins given without one.",
        params = { urls = "The URLs.", auths = "The logins, as Auth::parse reads them.", flag = "The flag's name, for errors.", cookie = "Whether a cookie may be named." },
    )]
    pub fn from_flags(urls: &[String], auths: &[String], flag: &str, cookie: bool) -> Result<Endpoints, String> {
        if auths.len() > urls.len() {
            return Err(format!("--{flag}-auth is given {} times, for {} --{flag}", auths.len(), urls.len()));
        }

        let list = urls
            .iter()
            .enumerate()
            .map(|(index, url)| {
                let auth = match auths.get(index) {
                    Some(text) => Auth::parse(text, cookie).map_err(|why| format!("--{flag}-auth for {}: {why}", host_of(url)))?,
                    None => Auth::None,
                };

                Ok(Endpoint { url: url.trim().to_string(), auth })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(Endpoints::new(list))
    }

    #[document(parent = "Endpoints", summary = "Whether it holds none.")]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    #[document(parent = "Endpoints", summary = "Its URLs, as they may be shown.")]
    pub fn hosts(&self) -> Vec<String> {
        self.list.iter().map(Endpoint::host).collect()
    }

    #[document(parent = "Endpoints", summary = "The endpoints themselves.")]
    pub fn list(&self) -> &[Endpoint] {
        &self.list
    }

    #[document(parent = "Endpoints", summary = "Asks the endpoint in use, and the others in turn while each fails; now is the time.", returns = "The first answer, or every endpoint's failure.")]
    pub fn ask<T>(&self, what: &str, ask: impl FnMut(&Endpoint) -> Result<T, EndpointFailure>) -> Result<T, String> {
        self.ask_at(Instant::now(), what, ask)
    }

    #[document(parent = "Endpoints", summary = "As ask, at a time given: what tests drive.")]
    pub fn ask_at<T>(&self, now: Instant, what: &str, mut ask: impl FnMut(&Endpoint) -> Result<T, EndpointFailure>) -> Result<T, String> {
        if self.list.is_empty() {
            return Err(format!("{what}: no URL given"));
        }

        let start = {
            let mut current = self.current.lock().expect("never poisoned");

            if current.since.is_some_and(|since| now.duration_since(since) >= self.back_after) {
                *current = Current { index: 0, since: None };
            }

            current.index
        };
        let mut failed = Vec::new();

        for step in 0..self.list.len() {
            let index = (start + step) % self.list.len();
            let endpoint = &self.list[index];

            match ask(endpoint) {
                Ok(answer) => {
                    let mut current = self.current.lock().expect("never poisoned");

                    if current.index != index {
                        if self.list.len() > 1 {
                            log::info!("{what}: using {} now", endpoint.host());
                        }

                        *current = Current { index, since: (index != 0).then_some(now) };
                    }

                    return Ok(answer);
                }
                // An answer is the service's, whichever URL gave it: said as the service says it.
                Err(EndpointFailure::Answer(why)) => return Err(format!("{what}: {why}")),
                Err(EndpointFailure::Endpoint(why)) => {
                    log::warn!("{what}: {} failed: {why}", endpoint.host());
                    failed.push(format!("{}: {why}", endpoint.host()));
                }
            }
        }

        Err(format!("{what}: {}", failed.join("; ")))
    }
}
