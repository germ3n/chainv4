//! The mining port over wss://, for a worker that keeps no browser: the node's WebSocket, GET
//! /mining/ws, behind TLS, carrying the port's protocol as it stands, a line a message. An open port's
//! token is public, so its handshake and MACs protect nothing from someone on the path, and this is
//! what does.
//!
//! TLS is rustls on ring, what the node's clients and the apps already speak, with the Mozilla roots
//! built in, so it reads the same on a desktop, Android and iOS. The certificate is always checked
//! against the host's name: nothing here turns that off. A node with a certificate of its own is
//! trusted by giving it, --ca-cert, and is checked the same way.

use {
    super::protocol::MOST_LINE,
    rustls::pki_types::{CertificateDer, ServerName, pem::PemObject},
    rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned},
    serde_json::Value,
    std::io::{Read, Write},
    std::net::TcpStream,
    std::sync::{Arc, mpsc},
    std::time::Duration,
    tungstenite::client::IntoClientRequest,
    tungstenite::handshake::HandshakeError,
    tungstenite::protocol::WebSocketConfig,
    tungstenite::{Message, WebSocket},
};

/// How long reaching the node over TLS and asking it for its WebSocket may take.
const SHAKING: Duration = Duration::from_secs(30);
/// How long a wait to read goes before what is waiting to be sent goes first. A nonce found waits at
/// most this.
const POLL: Duration = Duration::from_millis(25);
/// How many lines may wait to be sent, or read, before the side giving them waits too, as it would
/// on a full TCP connection.
const QUEUED: usize = 1024;
/// The most of GET /mining's answer that is read: far more than it says.
const MOST_ANSWER: u64 = 64 * 1024;

/// A worker's connection to the port once it is up: lines to send, and lines as they come.
pub struct Lines {
    pub out: mpsc::SyncSender<String>,
    pub came: mpsc::Receiver<Result<String, String>>,
}

/// Where a wss:// address leads: its host, its port, and itself for the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub url: String,
}

impl Target {
    /// Reads wss://host[:port][/path]: the host without an IPv6 address's brackets, and 443 unless
    /// told. Anything but wss:// is refused, ws:// by name, as it is no better than TCP.
    pub fn parse(url: &str) -> Result<Target, String> {
        if url.starts_with("ws://") {
            return Err(format!("{url} is a WebSocket without TLS, which protects nothing: give wss://"));
        }

        let rest = url.strip_prefix("wss://").ok_or_else(|| format!("{url} is not wss://host/path"))?;
        let (host, port) = authority(rest.split(['/', '?', '#']).next().unwrap_or_default(), 443).ok_or_else(|| format!("{url} names no host"))?;

        Ok(Target { host, port, url: url.to_string() })
    }

    /// Reads a node's API, http://host[:port] or https://, as where to connect to ask it, and whether
    /// that is over TLS.
    pub fn of_api(api: &str) -> Result<(Target, bool), String> {
        let (rest, port, tls) = match (api.strip_prefix("https://"), api.strip_prefix("http://")) {
            (Some(rest), _) => (rest, 443, true),
            (_, Some(rest)) => (rest, 80, false),
            _ => return Err(format!("{api} is not http:// or https://")),
        };
        let (host, port) = authority(rest.split(['/', '?', '#']).next().unwrap_or_default(), port).ok_or_else(|| format!("{api} names no host"))?;

        Ok((Target { host, port, url: api.to_string() }, tls))
    }

    /// Where to connect, host:port, as a proxy or the system is asked for it.
    pub fn address(&self) -> String {
        match self.host.contains(':') {
            true => format!("[{}]:{}", self.host, self.port),
            false => format!("{}:{}", self.host, self.port),
        }
    }
}

/// A URL's host:port, an IPv6 address's brackets gone, the port as told or the given one.
fn authority(text: &str, default: u16) -> Option<(String, u16)> {
    let text = text.rsplit_once('@').map_or(text, |(_, host)| host);
    let (host, port) = match text.strip_prefix('[') {
        Some(inside) => {
            let (host, after) = inside.split_once(']')?;

            (host, after.strip_prefix(':'))
        }
        None => match text.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (text, None),
        },
    };

    (!host.is_empty()).then_some(())?;

    Some((host.to_string(), port.map_or(Some(default), |port| port.parse().ok())?))
}

/// What a client checks a node's certificate against: the roots built in, or the certificates in the
/// PEM given instead. rustls's ring provider is named here, as what else is in the build could bring
/// another, and it would then not pick one.
fn tls_config(ca_cert: Option<&[u8]>) -> Result<Arc<ClientConfig>, String> {
    let mut roots = RootCertStore::empty();

    match ca_cert {
        None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
        Some(pem) => {
            let certs = CertificateDer::pem_slice_iter(pem).collect::<Result<Vec<_>, _>>().map_err(|err| format!("the certificate file does not read: {err}"))?;

            if certs.is_empty() {
                return Err("the certificate file holds no certificate".to_string());
            }

            for cert in certs {
                roots.add(cert).map_err(|err| format!("the certificate file: {err}"))?;
            }
        }
    }

    let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

/// TLS over a connection, its handshake done, so a certificate that does not check is said as that
/// and nothing is written past it.
fn tls_over(stream: TcpStream, host: &str, ca_cert: Option<&[u8]>) -> Result<StreamOwned<ClientConnection, TcpStream>, String> {
    let name = ServerName::try_from(host.to_string()).map_err(|_| format!("{host} is no name a certificate can be for"))?;
    let connection = ClientConnection::new(tls_config(ca_cert)?, name).map_err(|err| err.to_string())?;
    let mut tls = StreamOwned::new(connection, stream);
    tls.sock.set_read_timeout(Some(SHAKING)).map_err(|err| err.to_string())?;
    tls.sock.set_write_timeout(Some(SHAKING)).map_err(|err| err.to_string())?;

    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).map_err(|err| tls_failure(host, &err))?;
    }

    Ok(tls)
}

/// Why TLS did not come up: a certificate that does not check says so plainly, with what to do.
fn tls_failure(host: &str, err: &std::io::Error) -> String {
    match err.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
        Some(rustls::Error::InvalidCertificate(why)) => format!("the certificate {host} gave does not check: {why:?}. It is never skipped, as anyone on the way could then rewrite where this worker's work pays; a node with a certificate of its own takes --ca-cert FILE"),
        Some(why) => format!("TLS with {host} did not come up: {why}"),
        None => format!("TLS with {host} did not come up: {err}"),
    }
}

/// Opens the node's WebSocket over a connection already made to it, and runs it on a thread of its
/// own, as one TLS session cannot be read and written from two threads at once.
pub fn open(stream: TcpStream, target: &Target, ca_cert: Option<&[u8]>) -> Result<Lines, String> {
    let tls = tls_over(stream, &target.host, ca_cert)?;
    let request = target.url.as_str().into_client_request().map_err(|err| format!("{}: {err}", target.url))?;
    let config = WebSocketConfig::default().max_message_size(Some(MOST_LINE as usize)).max_frame_size(Some(MOST_LINE as usize));
    let (socket, _) = tungstenite::client::client_with_config(request, tls, Some(config)).map_err(|err| match err {
        HandshakeError::Failure(tungstenite::Error::Http(answer)) => match answer.status().as_u16() {
            404 => format!("{} is no mining WebSocket: the node takes no workers, or this is not the node", target.url),
            429 | 503 => format!("{} turned this worker away, with too many from this address already", target.url),
            status => format!("{} answered {status} rather than a WebSocket", target.url),
        },
        HandshakeError::Failure(err) => format!("{}: {err}", target.url),
        HandshakeError::Interrupted(_) => format!("{} took too long to answer", target.url),
    })?;
    socket.get_ref().sock.set_read_timeout(Some(POLL)).map_err(|err| err.to_string())?;
    socket.get_ref().sock.set_write_timeout(None).map_err(|err| err.to_string())?;

    let (out, to_send) = mpsc::sync_channel(QUEUED);
    let (to_take, came) = mpsc::sync_channel(QUEUED);

    std::thread::Builder::new().name("worker wss".to_string()).spawn(move || pump(socket, &to_send, &to_take)).map_err(|err| format!("the worker's connection did not start: {err}"))?;

    Ok(Lines { out, came })
}

/// Between the WebSocket and the worker, until either goes: what is waiting goes out, then a short
/// wait for what comes in. It ends once the worker drops its side, or the connection is shut under it.
fn pump<S: Read + Write>(mut socket: WebSocket<S>, to_send: &mpsc::Receiver<String>, to_take: &mpsc::SyncSender<Result<String, String>>) {
    loop {
        loop {
            match to_send.try_recv() {
                Ok(line) => {
                    if let Err(err) = socket.send(Message::text(line)) {
                        let _ = to_take.send(Err(format!("the connection broke: {err}")));

                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();

                    return;
                }
            }
        }

        match socket.read() {
            Ok(Message::Text(line)) => {
                if to_take.send(Ok(line.to_string())).is_err() {
                    return;
                }
            }
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                let _ = to_take.send(Err("the connection closed".to_string()));

                return;
            }
            // A ping is answered by the next flush, and the port sends nothing else.
            Ok(_) => {}
            Err(tungstenite::Error::Io(err)) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                let _ = socket.flush();
            }
            Err(err) => {
                let _ = to_take.send(Err(format!("the connection broke: {err}")));

                return;
            }
        }
    }
}

/// What a node says of its mining port, GET /mining, asked over a connection already made to it: over
/// TLS for an https:// node, whose certificate is checked as the WebSocket's is.
pub fn ask_mining(stream: TcpStream, host: &str, port: u16, tls: bool, ca_cert: Option<&[u8]>) -> Result<Value, String> {
    let name = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
    let host_header = match (tls, port) {
        (true, 443) | (false, 80) => name,
        _ => format!("{name}:{port}"),
    };

    stream.set_read_timeout(Some(SHAKING)).map_err(|err| err.to_string())?;
    stream.set_write_timeout(Some(SHAKING)).map_err(|err| err.to_string())?;

    match tls {
        true => answer_of(tls_over(stream, host, ca_cert)?, &host_header),
        false => answer_of(stream, &host_header),
    }
}

/// GET /mining over a stream: its answer's JSON, as the status says. HTTP/1.1, closed after, and
/// read to its end; a server may cut it into chunks all the same.
fn answer_of<S: Read + Write>(mut stream: S, host: &str) -> Result<Value, String> {
    let request = format!("GET /mining HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).and_then(|()| stream.flush()).map_err(|err| format!("asking {host}: {err}"))?;

    let mut answer = Vec::new();
    let read = stream.take(MOST_ANSWER).read_to_end(&mut answer);

    // Some servers end without TLS's own goodbye: what came is still all there is.
    if let Err(err) = read
        && answer.is_empty()
    {
        return Err(format!("asking {host}: {err}"));
    }

    let split = answer.windows(4).position(|end| end == b"\r\n\r\n").ok_or_else(|| format!("{host} gave no answer that reads"))?;
    let head = String::from_utf8_lossy(&answer[..split]).to_ascii_lowercase();
    let status: u16 = head.split_whitespace().nth(1).and_then(|status| status.parse().ok()).ok_or_else(|| format!("{host} gave no answer that reads"))?;

    match status {
        200 => {}
        404 => return Err(format!("{host} takes no workers: it was not started with --mining")),
        status => return Err(format!("{host} answered {status} to GET /mining")),
    }

    let body = &answer[split + 4..];
    let body = if head.lines().any(|line| line.starts_with("transfer-encoding:") && line.contains("chunked")) { unchunked(body).ok_or_else(|| format!("{host} gave no answer that reads"))? } else { body.to_vec() };

    serde_json::from_slice(&body).map_err(|_| format!("{host} gave an answer to GET /mining that does not read"))
}

/// A body cut into chunks, put back together; None for one that is not.
fn unchunked(mut body: &[u8]) -> Option<Vec<u8>> {
    let mut whole = Vec::new();

    loop {
        let end = body.windows(2).position(|end| end == b"\r\n")?;
        let size = usize::from_str_radix(std::str::from_utf8(&body[..end]).ok()?.split(';').next()?.trim(), 16).ok()?;

        if size == 0 {
            return Some(whole);
        }

        let start = end + 2;
        whole.extend_from_slice(body.get(start..start + size)?);
        body = body.get(start + size + 2..)?;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use {super::*, rustls::pki_types::PrivateKeyDer, std::net::TcpListener};

    /// A self-signed certificate for localhost alone, good for a hundred years, and its key, for
    /// tests: made with openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1.
    pub const CERT: &[u8] = include_bytes!("wss/test-cert.pem");
    const KEY: &[u8] = include_bytes!("wss/test-key.pem");

    /// One end of TLS for tests, as nginx is in front of a node.
    pub fn tls_listener() -> (TcpListener, Arc<rustls::ServerConfig>) {
        let certs = CertificateDer::pem_slice_iter(CERT).collect::<Result<Vec<_>, _>>().unwrap();
        let key = PrivateKeyDer::from_pem_slice(KEY).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap();

        (TcpListener::bind("127.0.0.1:0").unwrap(), Arc::new(config))
    }

    /// Takes one TLS connection, and asks what the worker's handshake with the given stream comes to.
    pub fn accept_one(listener: &TcpListener, config: &Arc<rustls::ServerConfig>) -> Result<StreamOwned<rustls::ServerConnection, TcpStream>, String> {
        let (stream, _) = listener.accept().map_err(|err| err.to_string())?;
        let mut tls = StreamOwned::new(rustls::ServerConnection::new(config.clone()).unwrap(), stream);

        while tls.conn.is_handshaking() {
            tls.conn.complete_io(&mut tls.sock).map_err(|err| err.to_string())?;
        }

        Ok(tls)
    }

    #[test]
    fn a_wss_address_reads_as_its_host_port_and_path_and_nothing_else_does() {
        assert_eq!(Target::parse("wss://pool.example/mining/ws").unwrap().address(), "pool.example:443");
        assert_eq!(Target::parse("wss://pool.example:8443/mining/ws").unwrap().address(), "pool.example:8443");
        assert_eq!(Target::parse("wss://[2001:db8::1]/mining/ws").unwrap().host, "2001:db8::1");
        assert_eq!(Target::parse("wss://[2001:db8::1]:8443/x").unwrap().address(), "[2001:db8::1]:8443");

        assert!(Target::parse("ws://pool.example/mining/ws").unwrap_err().contains("without TLS"));
        assert!(Target::parse("pool.example:4002").unwrap_err().contains("not wss://"));
        assert!(Target::parse("wss:///mining/ws").unwrap_err().contains("no host"));
        assert!(Target::parse("wss://host:port/").is_err());

        let (api, tls) = Target::of_api("https://pool.example/").unwrap();
        assert_eq!((api.address().as_str(), tls), ("pool.example:443", true));
        let (api, tls) = Target::of_api("http://[2001:db8::1]:8080").unwrap();
        assert_eq!((api.address().as_str(), tls), ("[2001:db8::1]:8080", false));
        assert_eq!(Target::of_api("http://pool.example").unwrap().0.port, 80);
        assert!(Target::of_api("ftp://pool.example").is_err());
    }

    /// The real thing on both sides: a WebSocket server behind a real TLS listener, lines going both
    /// ways through what a worker opens, and that a line too long is no line.
    #[test]
    fn lines_go_both_ways_over_a_real_tls_listener() {
        let (listener, config) = tls_listener();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || {
            let tls = accept_one(&listener, &config).unwrap();
            let mut socket = tungstenite::accept(tls).unwrap();
            socket.send(Message::text("the first line")).unwrap();
            assert_eq!(socket.read().unwrap(), Message::text("an answer"));
            assert_eq!(socket.read().unwrap(), Message::text("another"));
            socket.send(Message::text("the second line")).unwrap();
            // A ping is answered, as a proxy keeping the socket alive would want.
            socket.send(Message::Ping(vec![1, 2].into())).unwrap();
            assert_eq!(socket.read().unwrap(), Message::Pong(vec![1, 2].into()));
            socket.close(None).unwrap();

            while socket.read().is_ok() {}
        });

        let target = Target::parse(&format!("wss://localhost:{port}/mining/ws")).unwrap();
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let lines = open(stream, &target, Some(CERT)).unwrap();
        let wait = Duration::from_secs(10);

        assert_eq!(lines.came.recv_timeout(wait).unwrap().unwrap(), "the first line");
        lines.out.send("an answer".to_string()).unwrap();
        lines.out.send("another".to_string()).unwrap();
        assert_eq!(lines.came.recv_timeout(wait).unwrap().unwrap(), "the second line");
        assert!(lines.came.recv_timeout(wait).unwrap().unwrap_err().contains("closed"), "said once it goes");
        served.join().unwrap();
    }

    fn opened_against(host: &str, ca: Option<&[u8]>) -> String {
        let (listener, config) = tls_listener();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || accept_one(&listener, &config).map(|_| ()));
        let target = Target::parse(&format!("wss://{host}:{port}/mining/ws")).unwrap();
        let refused = match open(TcpStream::connect(("127.0.0.1", port)).unwrap(), &target, ca) {
            Ok(_) => panic!("it went through"),
            Err(why) => why,
        };

        assert!(served.join().unwrap().is_err(), "TLS never came up, so nothing reached the other side");

        refused
    }

    #[test]
    fn a_certificate_for_another_host_or_of_no_known_authority_is_refused() {
        // It is good, and for localhost: not for this name, whatever it is trusted for.
        let other = opened_against("pool.example", Some(CERT));
        assert!(other.contains("does not check") && other.contains("NotValidForName"), "{other}");
        let address = opened_against("127.0.0.1", Some(CERT));
        assert!(address.contains("does not check"), "{address}");

        // Self-signed, as a devnet node's, with nothing to trust it by: what its worker sees.
        let unknown = opened_against("localhost", None);
        assert!(unknown.contains("does not check") && unknown.contains("UnknownIssuer") && unknown.contains("--ca-cert"), "{unknown}");
    }

    #[test]
    fn a_certificate_file_that_holds_none_is_refused() {
        assert!(tls_config(Some(b"nothing of the kind")).unwrap_err().contains("no certificate"));
    }

    /// What answers GET /mining, TLS or not.
    fn answering(head: &'static str, body: &'static str) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut asked = [0; 512];
            let size = stream.read(&mut asked).unwrap();
            write!(stream, "{head}\r\n\r\n{body}").unwrap();

            asked[..size].to_vec()
        });

        (port, served)
    }

    #[test]
    fn get_mining_is_read_whole_or_cut_in_chunks_and_says_what_it_says() {
        let ask = |port: u16| ask_mining(TcpStream::connect(("127.0.0.1", port)).unwrap(), "127.0.0.1", port, false, None);

        let (port, served) = answering("HTTP/1.1 200 OK\r\nContent-Length: 13", r#"{"open":true}"#);
        assert_eq!(ask(port).unwrap()["open"], true);
        let asked = String::from_utf8(served.join().unwrap()).unwrap();
        assert!(asked.starts_with("GET /mining HTTP/1.1\r\n") && asked.contains(&format!("Host: 127.0.0.1:{port}\r\n")), "{asked}");

        let (port, served) = answering("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked", "7\r\n{\"open\"\r\n6\r\n:true}\r\n0\r\n\r\n");
        assert_eq!(ask(port).unwrap()["open"], true);
        served.join().unwrap();

        let (port, served) = answering("HTTP/1.1 404 Not Found", "");
        assert!(ask(port).unwrap_err().contains("takes no workers"));
        served.join().unwrap();

        let (port, served) = answering("no HTTP here", "");
        assert!(ask(port).is_err());
        served.join().unwrap();
    }

    #[test]
    fn get_mining_over_https_checks_the_certificate_as_the_websocket_does() {
        let (listener, config) = tls_listener();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || {
            let mut tls = accept_one(&listener, &config).unwrap();
            let mut asked = [0; 512];
            let _ = tls.read(&mut asked).unwrap();
            write!(tls, "HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n{{\"open\":true}}").unwrap();
            tls.conn.send_close_notify();
            let _ = tls.flush();
        });
        let connect = || TcpStream::connect(("127.0.0.1", port)).unwrap();

        assert_eq!(ask_mining(connect(), "localhost", port, true, Some(CERT)).unwrap()["open"], true);
        served.join().unwrap();

        let (listener, config) = tls_listener();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || accept_one(&listener, &config).map(|_| ()));
        let refused = ask_mining(TcpStream::connect(("127.0.0.1", port)).unwrap(), "pool.example", port, true, Some(CERT)).unwrap_err();
        assert!(refused.contains("does not check"), "{refused}");
        assert!(served.join().unwrap().is_err());
    }
}
