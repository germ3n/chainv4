//! The worker subcommand: mines for a node over its mining port, with no chain of its own.

use {
    super::CliResult,
    crate::mining::protocol::{OPEN_TOKEN, plain_cannot_be_read},
    crate::mining::worker::{Devices, Worker, WorkerConfig, connect_to},
    crate::mining::wss,
    serde_json::Value,
    std::path::PathBuf,
};

/// How long before asking a node where it takes workers again, while it does not answer.
const ASK_AGAIN: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(clap::Args)]
pub struct WorkerArgs {
    /// The node: its API, https://pool.example, which says where it takes workers; its mining port,
    /// host:port, such as 203.0.113.7:4002; or its WebSocket, wss://pool.example/mining/ws. A port
    /// open to anyone is reached over wss://, its token being public: over plain TCP only on this
    /// machine, at an onion through Tor here, or with --insecure-open-port.
    #[arg(long)]
    node: String,
    /// The file holding the token the node was given with --mining-token; none for an open port's.
    #[arg(long)]
    token: Option<PathBuf>,
    /// Mine on a port open to anyone over plain TCP, though anyone on the way could rewrite where this
    /// worker's work pays, both ways and unseen. On a network you trust, for a node with no TLS.
    #[arg(long)]
    insecure_open_port: bool,
    /// Trust the certificates in this PEM file for wss://, in place of the roots built in: a node's
    /// self-signed one, or a private CA's. Its host's name is checked all the same.
    #[arg(long, value_name = "FILE")]
    ca_cert: Option<PathBuf>,
    /// Where a pool pays this worker's shares, or --solo's blocks pay: an address, which a node
    /// running a pool needs.
    #[arg(long)]
    payout: Option<crate::structs::address::Address>,
    /// Mine alone: each block this worker finds pays --payout in full, on a node started with
    /// --mining-solo.
    #[arg(long, requires = "payout")]
    solo: bool,
    /// Where to keep the payout's claims on the pool, as the pool tells of each new one: with
    /// them, chainv4 claim --saved claims from any node, should the pool's be gone.
    #[cfg(feature = "pool")]
    #[arg(long, default_value = "pool-claims.json")]
    claims: PathBuf,
    /// Mine on the GPUs instead of CPU threads: every one, or those --gpus picks. Needs a build
    /// with the gpu feature.
    #[arg(long)]
    gpu: bool,
    /// The GPUs to mine on, by their number in chainv4 gpus, such as 0,2.
    #[arg(long, requires = "gpu", value_delimiter = ',')]
    gpus: Vec<usize>,
    /// How many CPU threads to mine on, without --gpu.
    #[arg(long, default_value_t = 1, conflicts_with = "gpu", value_parser = clap::value_parser!(u32).range(1..))]
    threads: u32,
    /// The node's protocol name, as its --protocol gives it: what its API says, or the default.
    #[arg(long, value_parser = crate::net::protocol_name)]
    protocol: Option<String>,
    /// What to call this worker in the node's report and on the explorer; the host name by default.
    #[arg(long)]
    name: Option<String>,
    /// Reach the node through a SOCKS5 proxy, socks5h://host:port: Tor's, socks5h://127.0.0.1:9050,
    /// for a node's onion, --node ADDRESS.onion:PORT.
    #[arg(long)]
    proxy: Option<String>,
}

/// Mines for the node until the process is stopped.
pub fn worker(args: WorkerArgs) -> CliResult<()> {
    let token = args.token.as_deref().map(crate::mining::read_token).transpose()?;
    let ca_cert = args.ca_cert.as_ref().map(|path| std::fs::read(path).map_err(|err| format!("--ca-cert {}: {err}", path.display()))).transpose()?;
    let (node, token, said_protocol) = route(&args.node, token, args.proxy.as_deref(), args.insecure_open_port, ca_cert.as_deref())?;
    let protocol = args.protocol.or(said_protocol).unwrap_or_else(|| crate::net::DEFAULT_PROTOCOL.to_string());
    let devices = match args.gpu {
        true => Devices::Gpu((!args.gpus.is_empty()).then_some(args.gpus)),
        false => Devices::Cpu(args.threads as usize),
    };

    #[cfg(not(feature = "gpu"))]
    if args.gpu {
        return Err("--gpu needs a build with the gpu feature: cargo build --features gpu".into());
    }

    let name = args.name.unwrap_or_else(host_name);
    log::info!("[worker] {name}: mining for {node}");

    Worker::start(WorkerConfig {
        node,
        token,
        name,
        protocol,
        devices,
        proxy: args.proxy,
        payout: args.payout,
        // A solo worker's blocks pay it directly: there is no claim to keep.
        #[cfg(feature = "pool")]
        claims: (!args.solo).then_some(args.claims),
        #[cfg(not(feature = "pool"))]
        claims: None,
        solo: args.solo,
        insecure_open_port: args.insecure_open_port,
        ca_cert,
    })?
    .wait();

    Ok(())
}

/// What a node's API says of its mining port. A node told outright as this one, that does not answer
/// yet, is asked again, as a worker keeps connecting to one that is away; one that answers wrongly, or
/// a certificate that does not check, is for a person to look at.
fn ask(api: &str, patient: bool, proxy: Option<&str>, ca_cert: Option<&[u8]>) -> Result<Value, String> {
    let (at, tls) = wss::Target::of_api(api)?;

    loop {
        match connect_to(&at.address(), proxy) {
            Ok(stream) => return wss::ask_mining(stream, &at.host, at.port, tls, ca_cert).map_err(|why| format!("{api}: {why}")),
            Err(why) if patient => {
                log::warn!("[worker] {api}: {why}; asking again in {}s", ASK_AGAIN.as_secs());
                std::thread::sleep(ASK_AGAIN);
            }
            Err(why) => return Err(format!("{api}: {why}")),
        }
    }
}

/// Where a node's API says its mining port is, for plain TCP: at its onion for an API reached at its
/// onion, where it publishes it, or at the API's host.
fn port_address(about: &Value, host: &str) -> Option<String> {
    let said = |name: &str| about[name].as_str().map(str::trim).filter(|said| !said.is_empty()).map(str::to_string);

    if crate::mining::protocol::is_onion(host)
        && let Some(onion) = said("onion")
    {
        return Some(onion);
    }

    said("public").or_else(|| {
        let port = about["port"].as_u64()?;

        Some(if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") })
    })
}

/// Where a worker told this goes, with which token and, if its API said, under what protocol name. Its
/// private token, or its way over plain TCP where that is safe or allowed, stays as it is; all else that
/// is open goes over the node's wss://, found as the wallet finds it, from GET /mining. Nothing here
/// falls back to plain TCP for not reaching that: Worker::start would refuse it as well.
fn route(node: &str, token: Option<Vec<u8>>, proxy: Option<&str>, insecure: bool, ca_cert: Option<&[u8]>) -> Result<(String, Vec<u8>, Option<String>), String> {
    let node = node.trim().trim_end_matches('/');

    if node.starts_with("wss://") {
        return Ok((node.to_string(), token.unwrap_or_else(|| OPEN_TOKEN.to_vec()), None));
    }

    // A node's mining port, told outright.
    if !node.contains("://") {
        return match token {
            Some(token) => Ok((node.to_string(), token, None)),
            None if insecure || plain_cannot_be_read(node, proxy) => Ok((node.to_string(), OPEN_TOKEN.to_vec(), None)),
            None => {
                let host = node.rsplit_once(':').map_or(node, |(host, _)| host);
                let api = format!("https://{host}");
                let about = ask(&api, false, proxy, ca_cert).map_err(|why| format!("{node} would be a port open to anyone, reached over wss://, and {why}. Give the node's API, --node https://NODE; --token for a private port; or --insecure-open-port for plain TCP all the same"))?;

                match about["open"] == true {
                    true => Ok((format!("wss://{}{}", host, socket_path(&about)), OPEN_TOKEN.to_vec(), said_protocol(&about))),
                    false => Err(format!("{node} is a private port: give --token, the file holding its token")),
                }
            }
        };
    }

    let about = ask(node, true, proxy, ca_cert)?;
    let (target, _) = wss::Target::of_api(node)?;
    let protocol = said_protocol(&about);
    let address = port_address(&about, &target.host).ok_or_else(|| format!("{node} said no mining port"))?;

    match (about["open"] == true, token) {
        (false, Some(token)) => Ok((address, token, protocol)),
        (false, None) => Err(format!("{node} is a private port: give --token, the file holding its token")),
        (true, _) if insecure || plain_cannot_be_read(&address, proxy) => Ok((address, OPEN_TOKEN.to_vec(), protocol)),
        (true, _) => match node.strip_prefix("https://") {
            Some(rest) => Ok((format!("wss://{rest}{}", socket_path(&about)), OPEN_TOKEN.to_vec(), protocol)),
            None => Err(format!("{node} takes anyone on a port whose token is public, and its API is not at https://, so there is no encrypted way to it: anyone on the way could rewrite where this worker's work pays. Reach it over https://, or give --insecure-open-port for plain TCP all the same")),
        },
    }
}

/// route, for a test of everything together in another module.
#[cfg(all(test, feature = "client", feature = "http"))]
pub(crate) fn route_for_tests(node: &str, proxy: Option<&str>, insecure: bool, ca_cert: Option<&[u8]>) -> Result<(String, Vec<u8>, Option<String>), String> {
    route(node, None, proxy, insecure, ca_cert)
}

/// The path a node's API says its WebSocket is at.
fn socket_path(about: &Value) -> String {
    let path = about["ws"].as_str().unwrap_or("/mining/ws");

    if path.starts_with('/') { path.to_string() } else { format!("/{path}") }
}

fn said_protocol(about: &Value) -> Option<String> {
    about["protocol"].as_str().map(str::to_string)
}

/// The machine's name, for a worker not given one.
fn host_name() -> String {
    ["HOSTNAME", "HOST", "COMPUTERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "worker".to_string())
}

/// Lists the GPUs this machine has, by the numbers --gpus takes.
#[cfg(feature = "gpu")]
pub fn gpus() -> CliResult<()> {
    let gpus = chainv4_gpu::list();

    if gpus.is_empty() {
        return Err("no GPU found".into());
    }

    for (index, name) in gpus.iter().enumerate() {
        println!("{index}: {name}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::mining::wss::tests::{CERT, accept_one, tls_listener},
        std::io::{Read, Write},
        std::net::TcpListener,
    };

    const TOKEN: &[u8] = b"a private port's token";
    /// A name nothing answers for, at once.
    const ELSEWHERE: &str = "pool.invalid:4002";

    /// A node's API over plain HTTP, answering one GET /mining with what is given.
    fn api(about: Value) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let at = format!("http://{}", listener.local_addr().unwrap());

        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut asked = [0; 1024];
            let _ = stream.read(&mut asked).unwrap();
            let body = about.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        });

        at
    }

    #[test]
    fn a_private_token_goes_over_plain_tcp_as_ever() {
        let routed = route(ELSEWHERE, Some(TOKEN.to_vec()), None, false, None).unwrap();

        assert_eq!(routed, (ELSEWHERE.to_string(), TOKEN.to_vec(), None));
    }

    #[test]
    fn an_open_port_on_this_machine_or_at_an_onion_through_tor_here_goes_over_plain_tcp() {
        let plain = |node: &str, proxy: Option<&str>| route(node, None, proxy, false, None).unwrap();
        let tor = Some("socks5h://127.0.0.1:9050");

        assert_eq!(plain("127.0.0.1:4002", None), ("127.0.0.1:4002".to_string(), OPEN_TOKEN.to_vec(), None));
        assert_eq!(plain("[::1]:4002", None).0, "[::1]:4002");
        assert_eq!(plain("abcdef.onion:4002", tor).0, "abcdef.onion:4002");
    }

    #[test]
    fn an_open_port_elsewhere_is_refused_unless_told_and_never_falls_back_to_plain_tcp() {
        let refused = route(ELSEWHERE, None, None, false, None).unwrap_err();

        assert!(refused.contains("--insecure-open-port") && refused.contains("https://"), "{refused}");
        assert_eq!(route(ELSEWHERE, None, None, true, None).unwrap().0, ELSEWHERE, "told, over plain TCP");

        // Tor on another machine reads what it is sent; an onion with no Tor goes by the system's resolver.
        assert!(route("abcdef.onion:4002", None, Some("socks5h://192.0.2.1:9050"), false, Some(CERT)).is_err());
    }

    #[test]
    fn a_nodes_api_at_https_leads_an_open_port_to_its_websocket_with_the_certificate_checked() {
        let (listener, config) = tls_listener();
        let port = listener.local_addr().unwrap().port();
        let served = std::thread::spawn(move || {
            let mut tls = accept_one(&listener, &config).unwrap();
            let mut asked = [0; 1024];
            let _ = tls.read(&mut asked).unwrap();
            let body = serde_json::json!({ "open": true, "port": 4002, "public": "pool.example:4002", "protocol": "mainnet", "ws": "/mining/ws" }).to_string();
            write!(tls, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            tls.conn.send_close_notify();
            let _ = tls.flush();
        });
        let (node, token, protocol) = route(&format!("https://localhost:{port}/"), None, None, false, Some(CERT)).unwrap();

        assert_eq!((node, token.as_slice(), protocol.as_deref()), (format!("wss://localhost:{port}/mining/ws"), OPEN_TOKEN, Some("mainnet")));
        served.join().unwrap();
    }

    #[test]
    fn a_nodes_api_at_http_gives_an_open_port_no_encrypted_way_and_says_so() {
        let open = || serde_json::json!({ "open": true, "port": 4002, "public": "pool.example:4002", "protocol": "chainv4" });
        let refused = route(&api(open()), None, None, false, None).unwrap_err();

        assert!(refused.contains("no encrypted way") && refused.contains("--insecure-open-port"), "{refused}");
        assert_eq!(route(&api(open()), None, None, true, None).unwrap().0, "pool.example:4002", "told, over plain TCP");

        // It publishes its port on this machine: nothing can read that.
        let here = serde_json::json!({ "open": true, "port": 4002, "public": null });
        assert_eq!(route(&api(here), None, None, false, None).unwrap().0, "127.0.0.1:4002");
    }

    #[test]
    fn a_nodes_api_leads_a_private_port_over_plain_tcp_with_its_token_or_asks_for_it() {
        let private = || serde_json::json!({ "open": false, "port": 4002, "public": "pool.example:4002", "protocol": "mainnet" });

        assert_eq!(route(&api(private()), Some(TOKEN.to_vec()), None, false, None).unwrap(), ("pool.example:4002".to_string(), TOKEN.to_vec(), Some("mainnet".to_string())));
        assert!(route(&api(private()), None, None, false, None).unwrap_err().contains("--token"));
    }

    #[test]
    fn a_websocket_told_outright_is_taken_as_it_is() {
        let (node, token, _) = route("wss://pool.example/mining/ws", None, None, false, None).unwrap();

        assert_eq!((node.as_str(), token.as_slice()), ("wss://pool.example/mining/ws", OPEN_TOKEN));
    }
}
