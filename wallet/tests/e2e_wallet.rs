//! End to end: the whole desktop wallet, driven headlessly through what accesskit sees of it
//! (egui_kittest), against a real chainv4 node process: restored from words, its balance proven
//! once the CLI pays it, and a send made from its Send page, which the node then holds.
//!
//! It needs the node's binary: CHAINV4_BIN, or the workspace's target/debug/chainv4 or
//! target/release/chainv4 (cargo build at the root first). Ignored by default:
//!
//!   cargo test --test e2e_wallet -- --ignored

use {
    egui_kittest::kittest::{NodeT, Queryable},
    egui_kittest::{Harness, Node as UiNode},
    std::io::{Read, Write},
    std::net::{TcpListener, TcpStream, UdpSocket},
    std::path::PathBuf,
    std::process::{Child, Command, Stdio},
    std::time::{Duration, Instant},
};

/// Hardhat's words: account 0's Ed25519 key is what the wallet restores from them.
const WORDS: &str = "test test test test test test test test test test test junk";

struct App(Box<dyn eframe::App>);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.ui(ui, frame);
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.0.logic(ctx, frame);
    }
}

fn binary() -> PathBuf {
    if let Some(path) = std::env::var_os("CHAINV4_BIN") {
        return PathBuf::from(path);
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");

    ["target/debug/chainv4", "target/release/chainv4"]
        .iter()
        .map(|path| root.join(path))
        .find(|path| path.exists())
        .unwrap_or_else(|| panic!("no chainv4 binary: cargo build at the workspace's root, or set CHAINV4_BIN"))
}

/// A node producing an authority chain, killed when dropped.
struct ChainNode {
    child: Child,
    http: u16,
}

impl ChainNode {
    fn start(dir: &std::path::Path) -> ChainNode {
        let http = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let p2p = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let log = std::fs::File::create(dir.join("node.log")).unwrap();
        let child = Command::new(binary())
            .args(["--consensus", "authority", "--produce", "--no-mdns", "--db"])
            .arg(dir.join("chain.db"))
            .args(["--listen", &format!("/ip4/127.0.0.1/udp/{p2p}/quic-v1"), "--http", &format!("127.0.0.1:{http}")])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();

        ChainNode { child, http }
    }

    fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.http)
    }

    fn get(&self, path: &str) -> Option<serde_json::Value> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.http)).ok()?;
        write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").ok()?;
        let mut answer = String::new();
        stream.read_to_string(&mut answer).ok()?;
        let (head, body) = answer.split_once("\r\n\r\n")?;

        head.starts_with("HTTP/1.1 200").then(|| serde_json::from_str(body).ok()).flatten()
    }

    fn balance(&self, address: &str) -> Option<String> {
        self.get(&format!("/accounts/{address}")).and_then(|account| account["balance"].as_str().map(str::to_string))
    }
}

impl Drop for ChainNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Everything accesskit shows that says something, a line each: what a failure prints.
fn shown(harness: &Harness<'_, App>) -> String {
    harness
        .root()
        .children_recursive()
        .filter_map(|node| {
            let access = node.accesskit_node();
            let said = [access.label().unwrap_or_default(), access.placeholder().map(str::to_string).unwrap_or_default(), access.value().unwrap_or_default()];

            said.iter().any(|text| !text.is_empty()).then(|| format!("{:?} {said:?}", access.role()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Steps the app, in real time as its requests come back, until done says so.
fn until(harness: &mut Harness<'_, App>, what: &str, seconds: u64, done: impl Fn(&Harness<'_, App>) -> bool) {
    let start = Instant::now();

    while start.elapsed() < Duration::from_secs(seconds) {
        harness.step();

        if done(harness) {
            return;
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("{what}: not within {seconds} s; the wallet shows:\n{}", shown(harness));
}

fn says(harness: &Harness<'_, App>, text: &str) -> bool {
    harness.root().children_recursive().any(|node| node.accesskit_node().value().is_some_and(|value| value.contains(text)))
}

fn field<'tree>(harness: &'tree Harness<'_, App>, hint: &'tree str) -> UiNode<'tree> {
    harness.get_by(|node| node.placeholder() == Some(hint))
}

fn type_into(harness: &mut Harness<'_, App>, hint: &str, text: &str) {
    field(harness, hint).click();
    harness.run_steps(2);
    field(harness, hint).type_text(text);
    harness.run_steps(2);
}

fn cli(args: &[&str]) -> String {
    let output = Command::new(binary()).args(args).output().unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "chainv4 {args:?}: {said}");

    said
}

#[test]
#[ignore = "drives the wallet against a real node for a minute or two: cargo test --test e2e_wallet -- --ignored"]
fn the_wallet_restores_proves_a_payment_and_sends_from_its_send_page() {
    use chainv4::crypto::keypair::KeyType;

    let dir = std::env::temp_dir().join(format!("chainv4-e2e-wallet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let node = ChainNode::start(&dir);
    let start = Instant::now();

    while node.get("/status").is_none() {
        assert!(start.elapsed() < Duration::from_secs(60), "the node did not come up");
        std::thread::sleep(Duration::from_millis(200));
    }

    let seed = chainv4::crypto::mnemonic::Phrase::parse(WORDS).unwrap().seed("");
    let mine = seed.key_at(KeyType::Ed25519, 0, 0).public().address().to_string();
    let bob = chainv4::crypto::keypair::Keypair::from_seed([9; 32]).public().address().to_string();

    let (folder, api) = (dir.join("wallet"), node.api());
    let mut harness = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| App(chainv4_wallet::wallet(cc, Some(api.clone()), &api, Some(folder.clone()))));
    harness.run_steps(3);

    // A first launch: an online wallet, restored from words, with no password.
    harness.get_by_label("Online wallet").click();
    harness.run_steps(3);
    harness.get_by_label("Restore from words").click();
    harness.run_steps(3);
    type_into(&mut harness, "word word word …", WORDS);
    harness.get_by_label("Restore").click();
    until(&mut harness, "the wallet opens on the restored account", 60, |harness| harness.query_all_by_label("Send").next().is_some());

    // Paid by the CLI, from the development key: the wallet shows the payment once it proved it.
    cli(&["send", "--node", &node.api(), "transfer", "--to", &mine, "--amount", "7000000000000000000"]);
    until(&mut harness, "the payment shows, proven", 90, |harness| says(harness, "7 DEV") && !says(harness, "Proving the balance"));
    assert!(!says(&harness, "5 DEV"), "only what was paid");

    // Its Send page sends 2 to bob, through the review.
    // The tab, of the two Sends there are (the Overview's button is the other).
    harness.get_all_by_label("Send").next().unwrap().click();
    harness.run_steps(5);
    type_into(&mut harness, "ed25519:0x…, an Ethereum 0x…, a bc1q…, or a name such as alice.dev", &bob);
    type_into(&mut harness, "0.0", "2");
    until(&mut harness, "the form can be reviewed", 30, |harness| harness.query_by_label("Review").is_some());
    harness.get_by_label("Review").click();
    until(&mut harness, "the review is priced", 60, |harness| harness.query_by_label("Sign and send").is_some());
    harness.get_by_label("Sign and send").click();

    let start = Instant::now();

    while node.balance(&bob).as_deref() != Some("2000000000000000000") {
        assert!(start.elapsed() < Duration::from_secs(90), "bob was never paid; the wallet shows:\n{}", shown(&harness));
        harness.step();
        std::thread::sleep(Duration::from_millis(50));
    }

    // The Send page says it landed; Overview shows what is left, proven: 7 less the 2 and the fee.
    until(&mut harness, "the send shows as proven", 30, |harness| says(harness, "Proven in block"));
    harness.get_all_by_label("Overview").next().unwrap().click();
    until(&mut harness, "what is left shows", 60, |harness| says(harness, "4.99") || says(harness, "4.9"));

    let _ = std::fs::remove_dir_all(&dir);
}
