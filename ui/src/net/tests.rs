use {
    super::*,
    std::io::{BufRead, BufReader, Read, Write},
    std::net::{SocketAddr, TcpListener, TcpStream},
    std::sync::atomic::{AtomicUsize, Ordering},
    std::sync::mpsc,
    std::sync::{Arc, Mutex},
    std::time::Duration,
};

#[test]
fn proxies_read_as_written_and_socks5_always_resolves_through_the_proxy() {
    let read = |text: &str| Proxy::parse(text).map(|proxy| proxy.as_str().to_string());

    assert_eq!(read(Proxy::ORBOT), Ok("socks5h://127.0.0.1:9050".to_string()));
    assert_eq!(read(" socks5://127.0.0.1:9050/ "), Ok("socks5h://127.0.0.1:9050".to_string()), "never a lookup outside the proxy");
    assert_eq!(read("SOCKS5H://tor.lan:9150"), Ok("socks5h://tor.lan:9150".to_string()));
    assert_eq!(read("http://10.0.0.2:8118"), Ok("http://10.0.0.2:8118".to_string()));
    assert_eq!(read("socks5h://me:secret@127.0.0.1:1080"), Ok("socks5h://me:secret@127.0.0.1:1080".to_string()));
    assert_eq!(Proxy::parse("socks5h://me:secret@127.0.0.1:1080").unwrap().shown(), "127.0.0.1:1080", "credentials stay out of messages");

    for (bad, why) in [
        ("127.0.0.1:9050", "socks5h://host:port"),
        ("https://proxy:443", "https proxies are not supported"),
        ("socks4://proxy:1080", "socks4 proxies are not supported"),
        ("socks5h://127.0.0.1", "give the proxy's port"),
        ("socks5h://127.0.0.1:0", "not a port"),
        ("socks5h://127.0.0.1:99999", "not a port"),
        ("socks5h://:9050", "not a proxy address"),
        ("socks5h://127.0.0.1:9050/path", "not a proxy address"),
    ] {
        assert!(Proxy::parse(bad).unwrap_err().contains(why), "{bad}: {:?}", Proxy::parse(bad));
    }
}

#[test]
fn settings_keep_one_spelling_and_an_empty_one_goes_direct() {
    assert_eq!(setting(""), Ok(String::new()));
    assert_eq!(setting("  "), Ok(String::new()));
    assert_eq!(setting(" socks5://127.0.0.1:9050 "), Ok(Proxy::ORBOT.to_string()));
    assert_eq!(setting("http://10.0.0.2:8118/"), Ok("http://10.0.0.2:8118".to_string()));
    assert!(setting("tor").is_err());
    assert!(Client::for_setting("").unwrap().proxy().is_none());
    assert_eq!(Client::for_setting(Proxy::ORBOT).unwrap().proxy().map(Proxy::as_str), Some(Proxy::ORBOT));
}

/// A node that answers every request with this body, counting the connections it takes.
fn node(body: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = Arc::new(AtomicUsize::new(0));
    let count = served.clone();

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let mut length = 0;

                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }

                    line.clear();
                }

                let mut sent = vec![0; length];
                let _ = reader.read_exact(&mut sent);
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });

    (address, served)
}

/// Copies both ways until either side closes.
fn relay(client: TcpStream, server: TcpStream) {
    let (mut client_read, mut server_write) = (client.try_clone().unwrap(), server.try_clone().unwrap());
    let (mut server_read, mut client_write) = (server, client);

    std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_read, &mut server_write);
        let _ = server_write.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut server_read, &mut client_write);
}

/// A SOCKS5 proxy that takes names only, as socks5h sends them, and sends every name to target:
/// a name the machine itself could not resolve still reaches it. It notes each name asked for.
fn socks5(target: SocketAddr) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = seen.clone();

            std::thread::spawn(move || {
                let mut client = stream;
                let mut greeting = [0; 2];
                client.read_exact(&mut greeting).unwrap();
                let mut methods = vec![0; greeting[1] as usize];
                client.read_exact(&mut methods).unwrap();
                client.write_all(&[5, 0]).unwrap();

                let mut request = [0; 4];
                client.read_exact(&mut request).unwrap();
                assert_eq!(request[3], 3, "socks5h sends the name, never an address it looked up");
                let mut length = [0; 1];
                client.read_exact(&mut length).unwrap();
                let mut name = vec![0; length[0] as usize];
                client.read_exact(&mut name).unwrap();
                let mut port = [0; 2];
                client.read_exact(&mut port).unwrap();
                seen.lock().unwrap().push(format!("{}:{}", String::from_utf8(name).unwrap(), u16::from_be_bytes(port)));

                let server = TcpStream::connect(target).unwrap();
                client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
                relay(client, server);
            });
        }
    });

    (address, asked)
}

/// An HTTP proxy that takes CONNECT, and sends every tunnel to target.
fn http_proxy(target: SocketAddr) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = seen.clone();

            std::thread::spawn(move || {
                let mut client = stream;
                let mut reader = BufReader::new(client.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut line = String::new();

                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    line.clear();
                }

                seen.lock().unwrap().push(first.trim().to_string());
                let server = TcpStream::connect(target).unwrap();
                client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
                relay(client, server);
            });
        }
    });

    (address, asked)
}

fn fetched(client: &Client, url: &str, body: Option<&str>) -> Result<Answer, String> {
    let (sent, answer) = mpsc::channel();
    client.fetch(url.to_string(), body.map(|body| body.as_bytes().to_vec()), move |result| sent.send(result).unwrap());

    answer.recv_timeout(Duration::from_secs(20)).unwrap()
}

#[test]
fn without_a_proxy_requests_go_straight_to_the_node() {
    let (node, served) = node("{\"height\":7}");
    let answer = fetched(&Client::direct(), &format!("http://{node}/status"), None).unwrap();

    assert_eq!((answer.ok, answer.status, answer.bytes), (true, 200, b"{\"height\":7}".to_vec()));
    assert_eq!(served.load(Ordering::SeqCst), 1);
}

#[test]
fn through_socks5_the_proxy_resolves_the_name_and_carries_everything() {
    let (node, served) = node("{\"ok\":true}");
    let (proxy, asked) = socks5(node);
    let client = Client::new(Some(Proxy::parse(&format!("socks5://{proxy}")).unwrap())).unwrap();

    // A name only the proxy knows, as a .onion address only Tor does.
    let got = fetched(&client, "http://chainv4-node.onion:8080/status", None).unwrap();
    let posted = fetched(&client, "http://chainv4-node.onion:8080/offer", Some("{\"tx\":\"0x00\"}")).unwrap();

    assert!(got.ok && posted.ok);
    assert_eq!(got.bytes, b"{\"ok\":true}");
    assert_eq!(asked.lock().unwrap()[0], "chainv4-node.onion:8080");
    assert!(served.load(Ordering::SeqCst) >= 1, "the node was reached, through the proxy");
}

#[test]
fn through_an_http_proxy_requests_tunnel_with_connect() {
    let (node, _) = node("{}");
    let (proxy, asked) = http_proxy(node);
    let client = Client::new(Some(Proxy::parse(&format!("http://{proxy}")).unwrap())).unwrap();
    let answer = fetched(&client, "http://chainv4-node.internal:8080/status", None).unwrap();

    assert!(answer.ok);
    assert!(asked.lock().unwrap()[0].starts_with("CONNECT chainv4-node.internal:8080 "), "{:?}", asked.lock().unwrap());
}

#[test]
fn with_the_proxy_down_nothing_goes_out_another_way() {
    let (node, served) = node("{}");
    // A port nothing listens on: the proxy, stopped.
    let stopped = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let client = Client::new(Some(Proxy::parse(&format!("socks5h://{stopped}")).unwrap())).unwrap();
    let failed = fetched(&client, &format!("http://{node}/status"), None).unwrap_err();

    assert!(failed.starts_with(&format!("through the proxy at {stopped}")), "{failed}");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(served.load(Ordering::SeqCst), 0, "the node never heard from us directly");

    let (sent, pieces) = mpsc::channel();
    client.stream(format!("http://{node}/stream"), move |piece| {
        sent.send(piece).unwrap();
        ControlFlow::Break(())
    });

    assert!(pieces.recv_timeout(Duration::from_secs(20)).unwrap().unwrap_err().contains("through the proxy"));
    assert_eq!(served.load(Ordering::SeqCst), 0);
}

#[test]
fn streams_come_through_the_proxy_head_first_then_their_body() {
    let (node, _) = node("event: block\ndata: 1\n\n");
    let (proxy, _) = socks5(node);
    let client = Client::new(Some(Proxy::parse(&format!("socks5h://{proxy}")).unwrap())).unwrap();
    let (sent, pieces) = mpsc::channel();

    client.stream("http://chainv4-node.onion/stream".to_string(), move |piece| {
        let end = matches!(piece, Ok(Piece::Chunk(ref bytes)) if bytes.is_empty()) || piece.is_err();
        sent.send(piece).unwrap();

        if end { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    });

    let mut body = Vec::new();
    let head = pieces.recv_timeout(Duration::from_secs(20)).unwrap().unwrap();

    assert_eq!(head, Piece::Head { ok: true, status: 200, status_text: "OK".to_string() });

    loop {
        match pieces.recv_timeout(Duration::from_secs(20)).unwrap().unwrap() {
            Piece::Chunk(bytes) if bytes.is_empty() => break,
            Piece::Chunk(bytes) => body.extend(bytes),
            Piece::Head { .. } => panic!("a second head"),
        }
    }

    assert_eq!(body, b"event: block\ndata: 1\n\n");
}
