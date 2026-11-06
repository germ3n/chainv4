//! Screenshots of every page of the explorer, at a desktop's size and a phone's, against a real
//! node, for looking the pages over: written to SHOTS_DIR (the system's temp folder unless set) as
//! explorer-WIDTHxHEIGHT-PAGE.png. Not a test of anything; ignored by default:
//!
//!   SHOTS_DIR=/tmp/shots cargo test --test shots -- --ignored --nocapture

use {
    egui_kittest::kittest::Queryable,
    egui_kittest::Harness,
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

fn cli(args: &[&str]) -> String {
    let output = Command::new(binary()).args(args).output().unwrap();

    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn settle(harness: &mut Harness<'_, App>, seconds: f64) {
    let start = Instant::now();

    while start.elapsed().as_secs_f64() < seconds {
        harness.step();
        std::thread::sleep(Duration::from_millis(30));
    }
}

const PAGES: [&str; 20] = ["Overview", "Block map", "Mempool", "Tokens", "NFTs", "Multisig", "Names", "Rich list & supply", "Markets", "DEX", "Swaps", "Bridges", "Network", "Finality", "Governance", "Mining pool", "Tools", "Send", "Faucet", "Settings"];
const SIZES: [(f32, f32); 2] = [(1280.0, 800.0), (390.0, 844.0)];
const PRESSING: (f32, f32) = (1280.0, 1600.0);

#[test]
#[ignore = "writes screenshots of every page: SHOTS_DIR=… cargo test --test shots -- --ignored"]
fn every_page_at_a_desktops_size_and_a_phones() {
    let out = std::env::var_os("SHOTS_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("chainv4-shots"));
    std::fs::create_dir_all(&out).unwrap();
    let dir = std::env::temp_dir().join(format!("chainv4-shots-explorer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let node = ChainNode::start(&dir);

    while node.get("/status").is_none() {
        std::thread::sleep(Duration::from_millis(200));
    }

    // Something to show: a few transfers.
    for n in 1..=3u8 {
        let to = chainv4::crypto::keypair::Keypair::from_seed([n; 32]).public().address().to_string();
        cli(&["send", "--node", &node.api(), "--no-wait", "transfer", "--to", &to, "--amount", &format!("{n}000000000000000000")]);
    }

    let (folder, api) = (dir.join("explorer"), node.api());
    let mut harness = Harness::builder().with_size(egui::vec2(PRESSING.0, PRESSING.1)).wgpu().build_eframe(|cc| App(chainv4_explorer::explorer(cc, Some(api.clone()), &api, Some(folder.clone()))));
    settle(&mut harness, 8.0);

    for page in PAGES {
        harness.set_size(egui::vec2(PRESSING.0, PRESSING.1));
        settle(&mut harness, 0.3);

        // A page shows once its group is open: each group's header opens or closes it.
        let button = |harness: &Harness<'_, App>| harness.query_all_by_role_and_label(egui::accesskit::Role::Button, page).next().map(|node| node.rect());
        let mut at = button(&harness);

        for group in ["Chain", "Markets", "Network", "Tools"] {
            if at.is_some() {
                break;
            }

            let found = harness.query_all_by_label(group).next().map(|header| header.click()).is_some();

            if found {
                settle(&mut harness, 0.4);
                at = button(&harness);

                if at.is_none() {
                    harness.query_all_by_label(group).next().unwrap().click();
                    settle(&mut harness, 0.4);
                }
            }
        }

        if at.is_none() {
            println!("no {page} to press");
            continue;
        }

        harness.query_all_by_role_and_label(egui::accesskit::Role::Button, page).next().unwrap().click();
        settle(&mut harness, 2.5);

        for (width, height) in SIZES {
            harness.set_size(egui::vec2(width, height));
            settle(&mut harness, 0.6);
            let image = harness.render().unwrap();
            let path = out.join(format!("explorer-{}x{}-{}.png", width as u32, height as u32, page.to_lowercase().replace([' ', '&'], "-")));
            image.save(&path).unwrap();
            println!("{}", path.display());
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
