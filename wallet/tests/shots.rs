//! Screenshots of every page of the wallet, at a desktop's size and a phone's, against a real node,
//! for looking the pages over: written to SHOTS_DIR (the system's temp folder unless set) as
//! wallet-WIDTHxHEIGHT-PAGE.png. Not a test of anything; ignored by default:
//!
//!   SHOTS_DIR=/tmp/shots cargo test --test shots -- --ignored --nocapture

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


/// Steps the app in real time for a while, so what it asked comes back.
fn settle(harness: &mut Harness<'_, App>, seconds: f64) {
    let start = Instant::now();

    while start.elapsed().as_secs_f64() < seconds {
        harness.step();
        std::thread::sleep(Duration::from_millis(30));
    }
}

const PAGES: [&str; 16] = ["Overview", "Send", "Batch", "Receive", "Activity", "NFTs", "Names", "Mining", "Multisig", "Governance", "Contracts", "Swaps", "Trade", "Bridge", "Accounts", "Settings"];
/// Tall enough that the sidebar shows every page to press.
const PRESSING: (f32, f32) = (1280.0, 1400.0);
const SIZES: [(f32, f32); 2] = [(1280.0, 800.0), (390.0, 844.0)];

#[test]
#[ignore = "writes screenshots of every page: SHOTS_DIR=… cargo test --test shots -- --ignored"]
fn every_page_at_a_desktops_size_and_a_phones() {
    let out = std::env::var_os("SHOTS_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("chainv4-shots"));
    std::fs::create_dir_all(&out).unwrap();
    let dir = std::env::temp_dir().join(format!("chainv4-shots-wallet-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let node = ChainNode::start(&dir);

    while node.get("/status").is_none() {
        std::thread::sleep(Duration::from_millis(200));
    }

    let seed = chainv4::crypto::mnemonic::Phrase::parse(WORDS).unwrap().seed("");
    let mine = seed.key_at(chainv4::crypto::keypair::KeyType::Ed25519, 0, 0).public().address().to_string();
    let (folder, api) = (dir.join("wallet"), node.api());
    let mut harness = Harness::builder().with_size(egui::vec2(SIZES[0].0, SIZES[0].1)).wgpu().build_eframe(|cc| App(chainv4_wallet::wallet(cc, Some(api.clone()), &api, Some(folder.clone()))));
    harness.run_steps(3);

    let shoot = |harness: &mut Harness<'_, App>, name: &str| {
        let image = harness.render().unwrap();
        let path = out.join(format!("wallet-{name}.png"));
        image.save(&path).unwrap();
        println!("{}", path.display());
    };
    shoot(&mut harness, "first-launch");
    harness.get_by_label("Online wallet").click();
    harness.run_steps(3);
    harness.get_by_label("Restore from words").click();
    harness.run_steps(3);
    shoot(&mut harness, "restore");
    type_into(&mut harness, "word word word …", WORDS);
    harness.get_by_label("Restore").click();
    until(&mut harness, "the wallet opens", 60, |harness| harness.query_all_by_label("Send").next().is_some());
    cli(&["send", "--node", &node.api(), "transfer", "--to", &mine, "--amount", "7000000000000000000"]);
    settle(&mut harness, 6.0);

    for page in PAGES {
        harness.set_size(egui::vec2(PRESSING.0, PRESSING.1));
        settle(&mut harness, 0.3);

        let Some(button) = harness.query_all_by_role_and_label(egui::accesskit::Role::Button, page).next() else {
            println!("no {page} to press");
            continue;
        };
        button.click();
        settle(&mut harness, 2.5);

        for (width, height) in SIZES {
            harness.set_size(egui::vec2(width, height));
            settle(&mut harness, 0.6);
            shoot(&mut harness, &format!("{}x{}-{}", width as u32, height as u32, page.to_lowercase()));
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
