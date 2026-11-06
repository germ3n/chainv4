//! End to end: the whole desktop explorer, driven headlessly through what accesskit sees of it
//! (egui_kittest), against a real chainv4 node process: its latest blocks, an account found by
//! searching it once the CLI pays it, its balance proven, and the block that paid it.
//!
//! It needs the node's binary: CHAINV4_BIN, or the workspace's target/debug/chainv4 or
//! target/release/chainv4 (cargo build at the root first). Ignored by default:
//!
//!   cargo test --test e2e_explorer -- --ignored

use {
    egui_kittest::kittest::{NodeT, Queryable},
    egui_kittest::{Harness, Node as UiNode},
    std::io::{Read, Write},
    std::net::{TcpListener, TcpStream, UdpSocket},
    std::path::PathBuf,
    std::process::{Child, Command, Stdio},
    std::time::{Duration, Instant},
};

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
#[ignore = "drives the explorer against a real node for a minute: cargo test --test e2e_explorer -- --ignored"]
fn the_explorer_lists_blocks_and_finds_an_account_and_the_block_that_paid_it() {
    let dir = std::env::temp_dir().join(format!("chainv4-e2e-explorer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let node = ChainNode::start(&dir);
    let start = Instant::now();

    while node.get("/status").is_none() {
        assert!(start.elapsed() < Duration::from_secs(60), "the node did not come up");
        std::thread::sleep(Duration::from_millis(200));
    }

    let (folder, api) = (dir.join("explorer"), node.api());
    let mut harness = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|cc| App(chainv4_explorer::explorer(cc, Some(api.clone()), &api, Some(folder.clone()))));

    // The latest blocks, each a link by its height.
    until(&mut harness, "the latest blocks list block 3", 60, |harness| harness.query_by_label("3").is_some());

    // An account paid by the CLI, found by searching its address: its balance, proven.
    let alice = chainv4::crypto::keypair::Keypair::from_seed([5; 32]).public().address().to_string();
    let said = cli(&["send", "--node", &node.api(), "transfer", "--to", &alice, "--amount", "5000000000000000000"]);
    let height: u64 = said.split("included in block ").nth(1).and_then(|rest| rest.split_whitespace().next()).and_then(|height| height.parse().ok()).unwrap_or_else(|| panic!("no block in {said}"));
    type_into(&mut harness, "Search a height, block or transaction id, or address", &alice);
    harness.key_press(egui::Key::Enter);
    until(&mut harness, "the account shows its balance", 60, |harness| says(harness, "5 DEV"));

    // Its block, searched by height, holds the one transaction.
    let search = "Search a height, block or transaction id, or address";
    field(&harness, search).click();
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    field(&harness, search).type_text(&height.to_string());
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    let (head, tail) = (alice[..16].to_string(), alice[alice.len() - 4..].to_string());
    until(&mut harness, "the block shows the transfer to alice", 60, |harness| {
        let to_alice = harness.root().children_recursive().any(|node| node.accesskit_node().label().is_some_and(|label| label.starts_with(&head) && label.ends_with(&tail)));

        says(harness, &format!("Block {height}")) && says(harness, "transfer") && to_alice
    });

    let _ = std::fs::remove_dir_all(&dir);
}
