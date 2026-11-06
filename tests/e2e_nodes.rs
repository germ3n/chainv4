//! End to end: real chainv4 processes on this machine's loopback, as people run them, each with
//! its own database, QUIC port and API, talking over real networking. What the in-process tests
//! cannot show: gossip and sync between processes, a node stopped and started again on its
//! database, one joining from a snapshot, and proof-of-work nodes that mined apart and meet.
//!
//! They take minutes, so they are ignored by default:
//!
//!   cargo test --release --test e2e_nodes -- --ignored --test-threads 1
//!
//! Each node's log is kept in its folder under the system's temp folder, named in a failure.

use {
    serde_json::Value,
    std::io::{Read, Write},
    std::net::{TcpListener, TcpStream, UdpSocket},
    std::path::{Path, PathBuf},
    std::process::{Child, Command, Stdio},
    std::time::{Duration, Instant},
};

const BINARY: &str = env!("CARGO_BIN_EXE_chainv4");

/// A port free on loopback now, of a kind.
fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn free_udp() -> u16 {
    UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A folder of its own for one test's nodes, gone with it unless the test failed.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("chainv4-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A node running, killed when dropped.
struct Node {
    name: String,
    dir: PathBuf,
    http: u16,
    p2p: u16,
    args: Vec<String>,
    child: Option<Child>,
}

impl Node {
    /// Starts one in its folder under the scratch one, with these arguments beside its own.
    fn start(scratch: &Scratch, name: &str, args: &[&str]) -> Node {
        let mut node = Node { name: name.to_string(), dir: scratch.0.join(name), http: free_tcp(), p2p: free_udp(), args: args.iter().map(|arg| arg.to_string()).collect(), child: None };
        std::fs::create_dir_all(&node.dir).unwrap();
        node.run();

        node
    }

    /// Where its peers dial it.
    fn address(&self) -> String {
        format!("/ip4/127.0.0.1/udp/{}/quic-v1", self.p2p)
    }

    fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.http)
    }

    fn run(&mut self) {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("node.log")).unwrap();
        let child = Command::new(BINARY)
            .args(["--db", &self.dir.join("chain.db").to_string_lossy(), "--listen", &self.address(), "--http", &format!("127.0.0.1:{}", self.http), "--no-mdns"])
            .args(&self.args)
            .env("RUST_LOG", "info")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        self.child = Some(child);
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn restart(&mut self) {
        self.stop();
        self.run();
    }

    /// GET a path of its API, as JSON; None while it does not answer.
    fn get(&self, path: &str) -> Option<Value> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.http)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").ok()?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer).ok()?;
        let (head, body) = answer.split_once("\r\n\r\n")?;

        head.starts_with("HTTP/1.1 200").then(|| serde_json::from_str(body).ok()).flatten()
    }

    fn status(&self) -> Option<Value> {
        self.get("/status")
    }

    fn height(&self) -> u64 {
        self.status().and_then(|status| status["height"].as_u64()).unwrap_or(0)
    }

    fn tip(&self) -> Option<String> {
        self.status().and_then(|status| status["tip"].as_str().map(str::to_string))
    }

    /// The block at a height, by its id, as this node has it.
    fn block_at(&self, height: u64) -> Option<String> {
        self.get(&format!("/blocks/{height}")).and_then(|block| block["id"].as_str().map(str::to_string))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("node.log")).unwrap_or_default()
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Waits for done, up to a while, or fails saying what it waited for, with the nodes' logs' ends.
fn until(what: &str, seconds: u64, nodes: &[&Node], done: impl Fn() -> bool) {
    let start = Instant::now();

    while start.elapsed() < Duration::from_secs(seconds) {
        if done() {
            return;
        }

        std::thread::sleep(Duration::from_millis(250));
    }

    let tails: Vec<String> = nodes
        .iter()
        .map(|node| {
            let log = node.log();
            let lines: Vec<&str> = log.lines().collect();

            format!("--- {} ({}) ---\n{}", node.name, node.dir.display(), lines[lines.len().saturating_sub(15)..].join("\n"))
        })
        .collect();

    panic!("{what}: not within {seconds} s\n{}", tails.join("\n"));
}

/// The CLI, run to its end: what it printed, or why it failed.
fn cli(args: &[&str]) -> Result<String, String> {
    let output = Command::new(BINARY).args(args).output().map_err(|err| err.to_string())?;
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));

    if output.status.success() { Ok(said) } else { Err(said) }
}

fn key_address(dir: &Path, name: &str) -> String {
    let file = dir.join(format!("{name}.key"));
    let said = cli(&["keygen", "--out", &file.to_string_lossy()]).unwrap();

    said.lines().find_map(|line| line.trim().strip_prefix("address ")).map(str::to_string).unwrap_or_else(|| panic!("keygen said no address: {said}"))
}

#[test]
#[ignore = "starts real nodes for a few minutes: cargo test --release --test e2e_nodes -- --ignored"]
fn nodes_produce_gossip_sync_finalize_restart_and_join_from_a_snapshot() {
    let scratch = Scratch::new("authority");
    let producer = Node::start(&scratch, "producer", &["--consensus", "authority", "--produce", "--validate"]);
    until("the producer serves its API", 60, &[&producer], || producer.status().is_some());
    let peer = producer.address();
    let mut followers: Vec<Node> = ["b", "c"].iter().map(|name| Node::start(&scratch, name, &["--consensus", "authority", "--peer", &peer])).collect();

    // Both follow the producer's chain, block for block.
    until("both followers reach height 6 on the producer's chain", 90, &[&producer, &followers[0], &followers[1]], || {
        followers.iter().all(|node| node.height() >= 6 && node.block_at(6).is_some() && node.block_at(6) == producer.block_at(6))
    });

    // A transfer sent to one follower is gossiped to the producer, which puts it in a block every
    // node then holds.
    let alice = key_address(&scratch.0, "alice");
    let sent = cli(&["send", "--node", &followers[0].api(), "--timeout", "60", "transfer", "--to", &alice, "--amount", "5"]);
    assert!(sent.is_ok(), "the transfer: {sent:?}");
    let balance = |node: &Node| node.get(&format!("/accounts/{alice}")).and_then(|account| account["balance"].as_str().map(str::to_string));
    until("every node holds the transfer", 60, &[&producer, &followers[0], &followers[1]], || {
        [&producer, &followers[0], &followers[1]].iter().all(|node| balance(node).as_deref() == Some("5"))
    });

    // FFG finalizes the first checkpoint, every 8 blocks with the producer voting, and the
    // followers see it final too.
    let finalized = |node: &Node| node.status().and_then(|status| status["finality"]["finalized"]["height"].as_u64()).unwrap_or(0);
    until("a follower sees a checkpoint finalized", 120, &[&producer, &followers[1]], || finalized(&followers[1]) >= 8);

    // Stopped while the chain goes on, a follower picks up where its database left off.
    followers[1].stop();
    let left_at = producer.height();
    until("the producer goes on without it", 60, &[&producer], || producer.height() >= left_at + 4);
    followers[1].restart();
    until("the follower started again catches up", 90, &[&producer, &followers[1]], || {
        let height = producer.height();
        followers[1].height() >= height.saturating_sub(1) && followers[1].block_at(left_at + 3) == producer.block_at(left_at + 3)
    });

    // A new node joins from a snapshot: the state a few blocks below the tip from its peers, and
    // only the blocks after run. It holds the transfer without having run its block.
    let joiner = Node::start(&scratch, "snapshot", &["--consensus", "authority", "--peer", &peer, "--snapshot", "--snapshot-depth", "4"]);
    until("the snapshot node reaches the tip and holds the transfer", 120, &[&producer, &joiner], || {
        joiner.height() + 1 >= producer.height() && balance(&joiner).as_deref() == Some("5")
    });
    assert!(joiner.status().unwrap()["bodies_from"].as_u64().unwrap() > 0, "it keeps bodies only from the snapshot on");

    followers.clear();
}

#[test]
#[ignore = "starts real miners for a few minutes: cargo test --release --test e2e_nodes -- --ignored"]
fn proof_of_work_nodes_that_mined_apart_meet_on_the_heaviest_chain() {
    let scratch = Scratch::new("pow");
    let a = Node::start(&scratch, "a", &["--mine", "--mine-threads", "1"]);
    let mut b = Node::start(&scratch, "b", &["--mine", "--mine-threads", "1"]);
    until("both mine a few blocks apart", 120, &[&a, &b], || a.height() >= 4 && b.height() >= 4);
    assert_ne!(a.block_at(1), b.block_at(1), "each mined its own chain");

    // b dials a: the lighter chain gives way, and both go on as one.
    b.args.extend(["--peer".to_string(), a.address()]);
    b.restart();
    until("both hold one chain", 180, &[&a, &b], || {
        let (Some(first), Some(second)) = (a.status(), b.status()) else { return false };
        let height = first["height"].as_u64().unwrap_or(0).min(second["height"].as_u64().unwrap_or(0)).saturating_sub(1);

        height >= 4 && a.block_at(height).is_some() && a.block_at(height) == b.block_at(height)
    });
    let tip = a.tip();
    assert!(tip.is_some());
}
