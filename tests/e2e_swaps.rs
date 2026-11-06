//! End to end: atomic swaps across real chains, each its own process, driven by the CLI as two
//! people would drive it: a chainv4 node, Foundry's anvil as an EVM chain, and Bitcoin Core on a
//! private regtest chain. Each swap is claimed on both sides; a lock nobody claims is refunded once
//! its deadline passes, which anvil and regtest let a test move time towards; and the Bitcoin lock
//! survives a reorganization under it.
//!
//! They need anvil and bitcoind on the path, and take a few minutes, so they are ignored:
//!
//!   cargo test --test e2e_swaps -- --ignored --test-threads 1

use {
    serde_json::{Value, json},
    std::io::{Read, Write},
    std::net::{TcpListener, TcpStream, UdpSocket},
    std::path::PathBuf,
    std::process::{Child, Command, Stdio},
    std::time::{Duration, Instant},
};

const BINARY: &str = env!("CARGO_BIN_EXE_chainv4");

fn free_tcp() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn until(what: &str, seconds: u64, done: impl Fn() -> bool) {
    let start = Instant::now();

    while start.elapsed() < Duration::from_secs(seconds) {
        if done() {
            return;
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    panic!("{what}: not within {seconds} s");
}

/// A process killed when dropped.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A folder of its own, kept when the test failed.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("chainv4-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        Scratch(dir)
    }

    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// One HTTP request to loopback, its body as JSON.
fn http(port: u16, method: &str, path: &str, body: Option<&Value>) -> Option<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let body = body.map(Value::to_string).unwrap_or_default();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).ok()?;
    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;
    let (_, body) = answer.split_once("\r\n\r\n")?;

    serde_json::from_str(body).ok()
}

/// The CLI, run to its end: what it printed, failing the test when it fails.
fn cli(args: &[&str]) -> String {
    let output = Command::new(BINARY).args(args).output().unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "chainv4 {}: {said}", args.join(" "));

    said
}

/// The same, again while it says a lock is not final yet, as swap lock and claim wait for a lock
/// on chainv4 to be final before they act on it: FFG finalizes every 8 blocks here.
fn cli_once_final(args: &[&str]) -> String {
    let start = Instant::now();

    loop {
        let output = Command::new(BINARY).args(args).output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));

        match output.status.success() {
            true => return said,
            false if said.contains("not final yet") && start.elapsed() < Duration::from_secs(120) => std::thread::sleep(Duration::from_secs(2)),
            false => panic!("chainv4 {}: {said}", args.join(" ")),
        }
    }
}

/// The first word of what was said that starts so.
fn word(said: &str, start: &str) -> String {
    said.split_whitespace().find(|word| word.starts_with(start)).map(|word| word.trim_matches(|c: char| c == '\'' || c == '"' || c == ',').to_string()).unwrap_or_else(|| panic!("nothing starting {start} in: {said}"))
}

/// What follows a label on its line, as keygen and swap secret write them: "secret 0x…".
fn after(said: &str, label: &str) -> String {
    said.lines().find_map(|line| line.trim().strip_prefix(label).map(|rest| rest.trim().to_string())).unwrap_or_else(|| panic!("no {label} in: {said}"))
}

/// A chainv4 node producing an authority chain, and the swap contract deployed on it.
struct Chain {
    _node: Running,
    http: u16,
    contract: String,
}

impl Chain {
    fn start(scratch: &Scratch) -> Chain {
        let http = free_tcp();
        let p2p = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let log = std::fs::File::create(scratch.0.join("node.log")).unwrap();
        let node = Running(
            Command::new(BINARY)
                .args(["--consensus", "authority", "--produce", "--validate", "--no-mdns", "--db", &scratch.file("chain.db")])
                .args(["--listen", &format!("/ip4/127.0.0.1/udp/{p2p}/quic-v1"), "--http", &format!("127.0.0.1:{http}")])
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        until("the node serves its API", 60, || http_get(http, "/status").is_some());
        let source = scratch.file("swap.lua");
        std::fs::write(&source, include_str!("../contracts/lua/swap.lua")).unwrap();
        let said = cli(&["send", "--node", &format!("http://127.0.0.1:{http}"), "deploy", "--lua", &source]);

        Chain { _node: node, http, contract: word(&said, "contract:0x") }
    }

    fn api(&self) -> String {
        format!("http://127.0.0.1:{}", self.http)
    }

    /// A new key, paid enough coin for its fees.
    fn funded_key(&self, scratch: &Scratch, name: &str) -> (String, String) {
        let file = scratch.file(&format!("{name}.key"));
        let address = after(&cli(&["keygen", "--out", &file]), "address");
        cli(&["send", "--node", &self.api(), "transfer", "--to", &address, "--amount", "100000000000000000000"]);

        (file, address)
    }
}

fn http_get(port: u16, path: &str) -> Option<Value> {
    http(port, "GET", path, None)
}

/// The development key's address, which signs when no --key is given.
fn dev_address() -> String {
    chainv4::crypto::keypair::Keypair::development().public().address().to_string()
}

/// Foundry's anvil, its JSON-RPC on a port of its own.
struct Anvil {
    _process: Running,
    port: u16,
}

impl Anvil {
    fn start() -> Anvil {
        let port = free_tcp();
        // A block a second, and an epoch of one slot, so its finalized block, two epochs back,
        // keeps up: swap lock and claim wait for the other's lock to be final there too.
        let process = Running(Command::new("anvil").args(["--port", &port.to_string(), "--silent", "--block-time", "1", "--slots-in-an-epoch", "1"]).spawn().expect("is Foundry's anvil installed?"));
        let anvil = Anvil { _process: process, port };
        until("anvil answers", 30, || anvil.rpc("eth_chainId", json!([])).is_some());

        anvil
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn rpc(&self, method: &str, params: Value) -> Option<Value> {
        http(self.port, "POST", "/", Some(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))).and_then(|answer| answer.get("result").cloned())
    }

    fn balance(&self, address: &str) -> u128 {
        let hex = self.rpc("eth_getBalance", json!([address, "latest"])).unwrap();

        u128::from_str_radix(hex.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    }

    /// Moves its clock on, and mines a block there.
    fn later(&self, seconds: u64) {
        self.rpc("evm_increaseTime", json!([seconds])).unwrap();
        self.rpc("evm_mine", json!([])).unwrap();
    }
}

/// A secp256k1 key, and ether set for it on anvil.
fn eth_key(scratch: &Scratch, anvil: &Anvil, name: &str) -> (String, String) {
    let file = scratch.file(&format!("{name}.key"));
    let said = cli(&["keygen", "--scheme", "secp256k1", "--out", &file]);
    let evm = after(&said, "evm");
    anvil.rpc("anvil_setBalance", json!([evm, "0x8ac7230489e80000"])).unwrap();

    (file, evm)
}

/// The code of a swap's offer or terms, in what the CLI printed.
fn code(said: &str) -> String {
    word(said, "UR:CHAINV4-SWAP/")
}

#[test]
#[ignore = "runs a chainv4 node and anvil: cargo test --test e2e_swaps -- --ignored"]
fn a_swap_between_chainv4_and_anvil_is_claimed_both_ways_and_one_left_is_refunded() {
    let scratch = Scratch::new("swap-evm");
    let chain = Chain::start(&scratch);
    let anvil = Anvil::start();
    let (bob_key, bob) = chain.funded_key(&scratch, "bob");
    let (alice_eth_key, alice_eth) = eth_key(&scratch, &anvil, "alice-eth");
    let (bob_eth_key, _) = eth_key(&scratch, &anvil, "bob-eth");
    cli(&["swap", "evm-deploy", "--eth-rpc", &anvil.url(), "--key", &bob_eth_key]);
    let node = chain.api();

    // Alice pays 1000 of chainv4's coin and gets 5000 wei on anvil; Bob the other way.
    let swap = |secret_said: &str| {
        let hash = after(secret_said, "hash");
        let offer = code(&cli(&["swap", "offer", "--hash", &hash, "--pay", "chainv4:1000", "--get", "evm:31337:5000", "--address", &dev_address(), "--evm-address", &alice_eth_key, "--contract", &chain.contract, "--node", &node]));

        code(&cli(&["swap", "answer", &offer, "--address", &bob, "--evm-address", &bob_eth_key, "--node", &node]))
    };
    let secret_said = cli(&["swap", "secret"]);
    let (secret, terms) = (after(&secret_said, "secret"), swap(&secret_said));

    // Alice locks first; Bob's lock checks hers stands before it goes in.
    cli(&["swap", "lock", &terms, "--node", &node]);
    cli_once_final(&["swap", "lock", &terms, "--key", &bob_eth_key, "--eth-rpc", &anvil.url(), "--node", &node]);

    // Alice claims the ether, which shows the secret on anvil; Bob reads it there and claims the coin.
    let before = anvil.balance(&alice_eth);
    cli_once_final(&["swap", "claim", &terms, "--secret", &secret, "--key", &alice_eth_key, "--eth-rpc", &anvil.url(), "--node", &node]);
    let status = cli(&["swap", "status", &terms, "--node", &node, "--eth-rpc", &anvil.url()]);
    assert!(status.contains(&secret[2..]) || status.contains(&secret), "the secret shows on anvil: {status}");
    cli_once_final(&["swap", "claim", &terms, "--secret", &secret, "--key", &bob_key, "--node", &node]);
    let status = cli(&["swap", "status", &terms, "--node", &node, "--eth-rpc", &anvil.url()]);
    assert_eq!(status.matches("claimed").count(), 2, "both legs claimed: {status}");
    assert!(anvil.balance(&alice_eth) + 1_000_000_000_000_000 > before, "alice's ether came, less her gas");

    // Another swap whose first lock goes unclaimed: past the second's deadline on anvil, Bob takes
    // his ether back.
    let secret_said = cli(&["swap", "secret"]);
    let terms = swap(&secret_said);
    cli(&["swap", "lock", &terms, "--node", &node]);
    cli_once_final(&["swap", "lock", &terms, "--key", &bob_eth_key, "--eth-rpc", &anvil.url(), "--node", &node]);
    anvil.later(13 * 3_600);
    cli(&["swap", "refund", &terms, "--key", &bob_eth_key, "--eth-rpc", &anvil.url(), "--node", &node, "--leg", "second"]);
    let status = cli(&["swap", "status", &terms, "--node", &node, "--eth-rpc", &anvil.url()]);
    assert!(status.contains("refunded"), "the second leg refunded: {status}");
}

/// Bitcoin Core on a private regtest chain, with a wallet that mines and pays.
struct Bitcoind {
    _process: Running,
    dir: PathBuf,
    rpc: u16,
}

impl Bitcoind {
    fn start(scratch: &Scratch) -> Bitcoind {
        let dir = scratch.0.join("bitcoind");
        std::fs::create_dir_all(&dir).unwrap();
        let (rpc, p2p) = (free_tcp(), free_tcp());
        let process = Running(
            Command::new("bitcoind")
                .args(["-regtest", "-txindex", "-fallbackfee=0.0002", "-listen=0", &format!("-datadir={}", dir.display()), &format!("-rpcport={rpc}"), &format!("-port={p2p}")])
                .stdout(Stdio::null())
                .spawn()
                .expect("is Bitcoin Core installed?"),
        );
        let bitcoind = Bitcoind { _process: process, dir, rpc };
        until("bitcoind answers", 60, || bitcoind.try_cli(&["getblockchaininfo"]).is_ok());
        bitcoind.cli(&["createwallet", "miner"]);
        bitcoind.mine(101);

        bitcoind
    }

    fn try_cli(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new("bitcoin-cli").args(["-regtest", &format!("-datadir={}", self.dir.display()), &format!("-rpcport={}", self.rpc)]).args(args).output().map_err(|err| err.to_string())?;

        match output.status.success() {
            true => Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()),
            false => Err(String::from_utf8_lossy(&output.stderr).to_string()),
        }
    }

    fn cli(&self, args: &[&str]) -> String {
        self.try_cli(args).unwrap_or_else(|why| panic!("bitcoin-cli {args:?}: {why}"))
    }

    fn mine(&self, blocks: u32) -> Vec<String> {
        let to = self.cli(&["getnewaddress"]);

        serde_json::from_str(&self.cli(&["generatetoaddress", &blocks.to_string(), &to])).unwrap()
    }
}

/// A test network address, tb1q…, as regtest writes it, bcrt1q…: the same program.
fn regtest_address(testnet: &str) -> String {
    let (_, program) = chainv4::structs::bech32::decode_segwit(testnet).unwrap();

    chainv4::structs::bech32::encode_segwit("bcrt", &program)
}

/// The output paying a script's address, in a transaction.
fn vout_to(bitcoind: &Bitcoind, txid: &str, address: &str) -> u32 {
    let tx: Value = serde_json::from_str(&bitcoind.cli(&["getrawtransaction", txid, "true"])).unwrap();

    tx["vout"].as_array().unwrap().iter().find(|out| out["scriptPubKey"]["address"].as_str() == Some(address)).and_then(|out| out["n"].as_u64()).unwrap() as u32
}

/// The longest hex word in what was said: a signed transaction.
fn raw_tx(said: &str) -> String {
    said.split_whitespace().filter(|word| word.len() > 100 && word.chars().all(|c| c.is_ascii_hexdigit())).max_by_key(|word| word.len()).unwrap_or_else(|| panic!("no transaction in: {said}")).to_string()
}

#[test]
#[ignore = "runs a chainv4 node and Bitcoin Core: cargo test --test e2e_swaps -- --ignored"]
fn a_swap_between_bitcoin_and_chainv4_survives_a_reorg_is_claimed_both_ways_and_one_left_is_refunded() {
    let scratch = Scratch::new("swap-btc");
    let chain = Chain::start(&scratch);
    let bitcoind = Bitcoind::start(&scratch);
    let (bob_key, bob) = chain.funded_key(&scratch, "bob");
    let btc_key = |name: &str| {
        let file = scratch.file(&format!("{name}.key"));
        let said = cli(&["keygen", "--scheme", "bitcoin", "--out", &file]);

        (file, after(&said, "testnet"))
    };
    let (alice_btc_key, alice_tb) = btc_key("alice-btc");
    let (bob_btc_key, bob_tb) = btc_key("bob-btc");
    let node = chain.api();

    // Alice pays 100,000 sats into the HTLC and gets 1000 of chainv4's coin.
    let swap = |secret_said: &str| {
        let hash = after(secret_said, "hash");
        let offer = code(&cli(&["swap", "offer", "--hash", &hash, "--pay", "bitcoin-testnet:100000", "--get", "chainv4:1000", "--bitcoin-key", &alice_btc_key, "--address", &dev_address(), "--contract", &chain.contract, "--node", &node]));
        let terms = code(&cli(&["swap", "answer", &offer, "--bitcoin-key", &bob_btc_key, "--address", &bob, "--node", &node]));
        let htlc = regtest_address(&word(&cli(&["swap", "show", &terms]), "tb1q"));

        (terms, htlc)
    };
    let fund = |htlc: &str| {
        let txid = bitcoind.cli(&["sendtoaddress", htlc, "0.001"]);
        let mined = bitcoind.mine(1);

        (txid, mined[0].clone())
    };
    let secret_said = cli(&["swap", "secret"]);
    let secret = after(&secret_said, "secret");
    let (terms, htlc) = swap(&secret_said);
    let (funding, block) = fund(&htlc);

    // The block holding the HTLC's funding is taken back; the funding goes back to the mempool and
    // into a block of the branch that replaces it, at the same output.
    bitcoind.cli(&["invalidateblock", &block]);
    let raw = bitcoind.cli(&["getrawtransaction", &funding]);
    assert!(bitcoind.cli(&["getrawmempool"]).contains(&funding), "back in the mempool");
    bitcoind.mine(2);
    assert_ne!(bitcoind.cli(&["getblockhash", "102"]), block, "another branch");
    assert_eq!(bitcoind.cli(&["getrawtransaction", &funding]), raw);
    let vout = vout_to(&bitcoind, &funding, &htlc);

    // Bob, seeing it confirmed, locks the coin, unable to check Bitcoin himself; Alice claims it,
    // which shows the secret on chainv4, and Bob claims the bitcoin with it.
    cli(&["swap", "lock", &terms, "--key", &bob_key, "--anyway", "--node", &node]);
    cli_once_final(&["swap", "claim", &terms, "--secret", &secret, "--node", &node]);
    let status = cli(&["swap", "status", &terms, "--node", &node]);
    assert!(status.contains(&secret[2..]) || status.contains(&secret), "the secret shows on chainv4: {status}");
    let claim = raw_tx(&cli(&["swap", "bitcoin-claim", &terms, "--secret", &secret, "--funding", &format!("{funding}:{vout}"), "--sats", "100000", "--key", &bob_btc_key, "--to", &bob_tb, "--fee-rate", "2"]));
    let claimed = bitcoind.cli(&["sendrawtransaction", &claim]);
    bitcoind.mine(1);
    let found: Value = serde_json::from_str(&bitcoind.cli(&["getrawtransaction", &claimed, "true"])).unwrap();
    assert!(found["confirmations"].as_u64().unwrap() >= 1, "Bob's claim is mined");
    assert_eq!(found["vout"][0]["scriptPubKey"]["address"].as_str(), Some(regtest_address(&bob_tb).as_str()), "to Bob");

    // Another swap Bob never locks: once Bitcoin's median time passes the HTLC's lock time, Alice
    // takes her bitcoin back.
    let secret_said = cli(&["swap", "secret"]);
    let (terms, htlc) = swap(&secret_said);
    let (funding, _) = fund(&htlc);
    let vout = vout_to(&bitcoind, &funding, &htlc);
    let refund = raw_tx(&cli(&["swap", "bitcoin-refund", &terms, "--funding", &format!("{funding}:{vout}"), "--sats", "100000", "--key", &alice_btc_key, "--to", &alice_tb, "--fee-rate", "2"]));
    assert!(bitcoind.try_cli(&["sendrawtransaction", &refund]).unwrap_err().contains("non-final"), "refused before its lock time");
    let later = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 26 * 3_600;
    bitcoind.cli(&["setmocktime", &later.to_string()]);
    bitcoind.mine(12);
    let refunded = bitcoind.cli(&["sendrawtransaction", &refund]);
    bitcoind.mine(1);
    let found: Value = serde_json::from_str(&bitcoind.cli(&["getrawtransaction", &refunded, "true"])).unwrap();
    assert!(found["confirmations"].as_u64().unwrap() >= 1, "Alice's refund is mined");
}
