//! A Bitcoin light client, on the desktop and Android: Kyoto (the bip157 crate) asks Bitcoin's own
//! peers for each block's compact filter (BIP157/158), the wallet tests every filter against the
//! scripts it watches, and fetches only the blocks that match. No server learns which addresses
//! are the wallet's, and nothing it says is taken on trust: filters are checked against the
//! headers' chain of work.
//!
//! It watches what the Settings switches ask for, on Bitcoin and on one of its test networks:
//! each account's own Bitcoin addresses, for their balances, and every swap's HTLC, for the coins
//! paying it and the claim that shows its secret. It syncs from a checkpoint built in, a block a
//! week or so before the build, then from where it last got to, which the wallet keeps between
//! runs: a first sync takes some minutes and a few tens of megabytes, later ones the blocks since.
//! Its connections go through the wallet's proxy when one is set: SOCKS5 alone, as Tor's is.

// Broadcasting, and reading a claim's secret, are the Swaps page's.
#![cfg_attr(not(feature = "swap"), allow(dead_code))]

use {
    bip157::chain::BlockHeaderChanges,
    bip157::tokio,
    bip157::{Block, BlockHash, Builder, ChainState, Client, Event, HashCheckpoint, Info, Network, ScriptBuf, Socks5Proxy, Transaction, TrustedPeer},
    chainv4::crypto::bitcoin::sha256,
    serde::{Deserialize, Serialize},
    std::collections::BTreeSet,
    std::net::SocketAddr,
    std::sync::{Arc, Mutex},
};

/// Which Bitcoin network a light client follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Net {
    Main,
    Testnet4,
    Signet,
    /// A private chain, for tests: from its genesis, and only from the peer named.
    Regtest,
}

impl Net {
    pub fn name(self) -> &'static str {
        match self {
            Net::Main => "Bitcoin",
            Net::Testnet4 => "testnet4",
            Net::Signet => "signet",
            Net::Regtest => "regtest",
        }
    }

    /// Whether its addresses are tb1q…, Bitcoin's test networks'.
    pub fn testnet(self) -> bool {
        self != Net::Main
    }

    /// The test network a setting names: signet, or else testnet4.
    pub fn test(setting: &str) -> Net {
        match setting.trim() {
            "signet" => Net::Signet,
            _ => Net::Testnet4,
        }
    }

    fn network(self) -> Network {
        match self {
            Net::Main => Network::Bitcoin,
            Net::Testnet4 => Network::Testnet4,
            Net::Signet => Network::Signet,
            Net::Regtest => Network::Regtest,
        }
    }

    /// Where a first sync begins: a block a week or so before the build, its hash checked with
    /// two block explorers; the genesis for a private chain.
    fn checkpoint(self) -> HashCheckpoint {
        let at = |height: u32, hash: &str| HashCheckpoint::new(height, hash.parse::<BlockHash>().expect("a block hash"));

        match self {
            Net::Main => at(972_410, "00000000000000000001ba187a086ecd0029f8ac4bc7b445822216850844a9ce"),
            Net::Testnet4 => at(156_773, "00000000baa17a29b67eee43bc811378946e986c7d1eb6d27709fb94558decb2"),
            Net::Signet => at(327_246, "0000000d7f613dec13839b557f17143517390267746543e9340a7588c0695526"),
            Net::Regtest => HashCheckpoint::from_genesis(Network::Regtest),
        }
    }
}

/// What a light client found, kept between runs: where it got to, and the coins paying the
/// scripts it watches, with what spent them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    /// The last block scanned, by height and hash: the next run begins after it.
    pub height: u32,
    pub hash: String,
    pub coins: Vec<Coin>,
}

/// An output paying a watched script.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coin {
    /// Its transaction's id, as explorers write it, and its index there.
    pub txid: String,
    pub vout: u32,
    /// What it holds, in satoshis.
    pub value: u64,
    /// The script it pays, in hex.
    pub script: String,
    /// The block it was mined in.
    pub height: u32,
    pub spent: Option<Spent>,
    /// What shows it to a contract that follows Bitcoin's headers, as the bridge's Bitcoin
    /// gateway does: kept from the block it came in. None for coins found before it was kept.
    #[serde(default)]
    pub proof: Option<CoinProof>,
}

/// A coin's transaction in its block: the block's hash and the transaction without its witness,
/// in hex, its place there, and its Merkle path, each sibling in Bitcoin's own byte order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoinProof {
    pub block: String,
    pub raw: String,
    pub index: u32,
    pub path: String,
}

/// What spent a coin: the transaction, its block, and the input's witness, each item in hex.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spent {
    pub txid: String,
    pub height: u32,
    pub witness: Vec<String>,
}

/// How a light client is doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The headers' height, as the peers' chain of most work has it.
    pub headers: u32,
    /// The last block whose filter was checked.
    pub filters: u32,
    /// Whether it has as many peers as it wants.
    pub connected: bool,
    /// Whether it has checked every filter to the tip.
    pub synced: bool,
    pub warning: Option<String>,
    /// Why it stopped, when it did.
    pub stopped: Option<String>,
}

#[derive(Default)]
struct Shared {
    seen: Seen,
    status: Status,
    watched: BTreeSet<Vec<u8>>,
    /// What came of broadcasts, by the number each was asked under.
    broadcast: Vec<(u64, Result<String, String>)>,
}

enum Command {
    Broadcast(u64, Vec<u8>),
}

/// A light client running on a thread of its own, until dropped.
pub struct Light {
    pub net: Net,
    /// What it was started with, so a change of settings starts it again.
    pub started_with: (Option<SocketAddr>, Option<String>),
    shared: Arc<Mutex<Shared>>,
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    next: u64,
}

impl Light {
    /// Starts one: from where it last got to, or from the checkpoint, watching these scripts.
    pub fn start(net: Net, seen: Seen, proxy: Option<SocketAddr>, peer: Option<String>, watched: BTreeSet<Vec<u8>>, wake: egui::Context) -> Light {
        let shared = Arc::new(Mutex::new(Shared { seen, watched, ..Shared::default() }));
        let (commands, received) = tokio::sync::mpsc::unbounded_channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (inner, trusted) = (shared.clone(), peer.clone());

        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                Ok(runtime) => runtime,
                Err(err) => {
                    lock(&inner).status.stopped = Some(format!("no runtime: {err}"));
                    return;
                }
            };

            runtime.block_on(run(net, inner, proxy, trusted, received, stopped, wake));
        });

        Light { net, started_with: (proxy, peer), shared, commands, stop: Some(stop), next: 0 }
    }

    /// The scripts it looks for from here on. Ones added later are found in blocks from then:
    /// new addresses and HTLCs have nothing paid to them before.
    pub fn watch(&self, scripts: BTreeSet<Vec<u8>>) {
        lock(&self.shared).watched = scripts;
    }

    pub fn status(&self) -> Status {
        lock(&self.shared).status.clone()
    }

    pub fn seen(&self) -> Seen {
        lock(&self.shared).seen.clone()
    }

    /// The tip as far as it knows: its headers', or what it scanned.
    pub fn tip(&self) -> u32 {
        let shared = lock(&self.shared);

        shared.status.headers.max(shared.seen.height)
    }

    /// How deep a block is: 1 for the tip.
    pub fn confirmations(&self, height: u32) -> u32 {
        (self.tip() + 1).saturating_sub(height)
    }

    /// The coins paying a script, spent or not.
    pub fn coins(&self, script: &[u8]) -> Vec<Coin> {
        let script = hex(script);

        lock(&self.shared).seen.coins.iter().filter(|coin| coin.script == script).cloned().collect()
    }

    /// The secret a claim of an HTLC showed: a witness item of 32 bytes whose SHA-256 is the hash.
    pub fn secret(&self, script: &[u8], hash: &[u8; 32]) -> Option<[u8; 32]> {
        secret_in(&lock(&self.shared).seen, script, hash)
    }

    /// Sends a signed transaction to Bitcoin's peers: the number to ask for what came of it by.
    pub fn broadcast(&mut self, raw: Vec<u8>) -> u64 {
        self.next += 1;
        let id = self.next;

        if self.commands.send(Command::Broadcast(id, raw)).is_err() {
            lock(&self.shared).broadcast.push((id, Err("the light client stopped".to_string())));
        }

        id
    }

    /// What came of a broadcast: its txid, or why it failed, once known.
    pub fn broadcasted(&self, id: u64) -> Option<Result<String, String>> {
        lock(&self.shared).broadcast.iter().find(|(asked, _)| *asked == id).map(|(_, result)| result.clone())
    }
}

impl Drop for Light {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// The light clients the settings ask for, at most one on Bitcoin and one on a test network,
/// and what each found, kept between runs.
#[derive(Default)]
pub struct Lights {
    pub main: Option<Light>,
    pub test: Option<Light>,
    /// What each network's client found, as of the last save or stop.
    pub kept: std::collections::HashMap<Net, Seen>,
    /// Why one would not start: a proxy that is not SOCKS5.
    pub trouble: Option<String>,
}

impl Lights {
    /// Starts the clients wanted, stops the others, starts one again when its proxy or peer
    /// changed, and has each watch its network's scripts.
    pub fn keep(&mut self, wanted: [(Net, bool); 2], proxy_setting: &str, peer: &str, scripts: impl Fn(Net) -> BTreeSet<Vec<u8>>, wake: &egui::Context) {
        let proxy = match proxy(proxy_setting) {
            Ok(proxy) => {
                self.trouble = None;
                proxy
            }
            Err(why) => {
                self.trouble = Some(why);
                self.stop_all();

                return;
            }
        };
        let peer = Some(peer.trim().to_string()).filter(|peer| !peer.is_empty());

        for (slot, (net, on)) in [false, true].into_iter().zip(wanted) {
            let running = if slot { &mut self.test } else { &mut self.main };
            let stale = running.as_ref().is_some_and(|light| !on || light.net != net || light.started_with != (proxy, peer.clone()));

            if stale && let Some(light) = running.take() {
                self.kept.insert(light.net, light.seen());
            }

            match running {
                Some(light) => light.watch(scripts(net)),
                None if on => *running = Some(Light::start(net, self.kept.get(&net).cloned().unwrap_or_default(), proxy, peer.clone(), scripts(net), wake.clone())),
                None => {}
            }
        }
    }

    /// The client following Bitcoin, or its test network.
    pub fn on(&self, testnet: bool) -> Option<&Light> {
        if testnet { self.test.as_ref() } else { self.main.as_ref() }
    }

    pub fn on_mut(&mut self, testnet: bool) -> Option<&mut Light> {
        if testnet { self.test.as_mut() } else { self.main.as_mut() }
    }

    /// What the running clients found, into what is kept.
    pub fn sync(&mut self) {
        for light in [&self.main, &self.test].into_iter().flatten() {
            self.kept.insert(light.net, light.seen());
        }
    }

    pub fn stop_all(&mut self) {
        self.sync();
        (self.main, self.test) = (None, None);
    }

    /// Forgets what a network's client found, and starts it again from the checkpoint.
    pub fn rescan(&mut self, testnet: bool) {
        let slot = if testnet { &mut self.test } else { &mut self.main };

        if let Some(light) = slot.take() {
            self.kept.remove(&light.net);
        }
    }
}

/// The scripts a light client on Bitcoin, or its test network, watches: each account's own
/// address (its bitcoin key's on Bitcoin, its words' testnet key on the test networks), and every
/// swap's HTLC on that network.
pub fn scripts(holders: &[crate::keys::Holder], book: &crate::vault::Book, testnet: bool) -> BTreeSet<Vec<u8>> {
    #[cfg_attr(not(feature = "swap"), allow(unused_mut))]
    let mut scripts = account_scripts(holders, book, testnet);

    #[cfg(feature = "swap")]
    for record in &book.swaps {
        if let Ok(terms) = chainv4::swap::Terms::from_text(&record.terms)
            && let (Some(htlc), Some((_, leg))) = (terms.htlc(), terms.bitcoin())
            && leg.testnet == testnet
        {
            scripts.insert(htlc.output());
        }
    }

    scripts
}

/// The accounts' own addresses' scripts alone.
pub fn account_scripts(holders: &[crate::keys::Holder], book: &crate::vault::Book, testnet: bool) -> BTreeSet<Vec<u8>> {
    use chainv4::crypto::bitcoin::{hash160, tx::{Network as BitcoinNetwork, p2wpkh}};
    use chainv4::crypto::keypair::PublicKey;

    let mut scripts = BTreeSet::new();
    let network = if testnet { BitcoinNetwork::Test } else { BitcoinNetwork::Main };

    for holder in holders {
        match testnet {
            false => scripts.extend(holder.publics.iter().filter_map(|public| match public {
                PublicKey::Bitcoin(key) => Some(p2wpkh(&hash160(key))),
                _ => None,
            })),
            true => {
                if let (Some(seed), Some(number)) = (crate::keys::seed(book), holder.number())
                    && let PublicKey::Bitcoin(key) = seed.bitcoin_key(network.coin(), 0, number).public()
                {
                    scripts.insert(p2wpkh(&hash160(&key)));
                }
            }
        }
    }

    scripts
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A SOCKS5 proxy's address, from the wallet's proxy setting: none for none. A proxy that is not
/// SOCKS5 is refused rather than gone around, so nothing connects in the clear by surprise.
pub fn proxy(setting: &str) -> Result<Option<SocketAddr>, String> {
    let setting = setting.trim();

    if setting.is_empty() {
        return Ok(None);
    }

    let address = setting
        .strip_prefix("socks5h://")
        .or_else(|| setting.strip_prefix("socks5://"))
        .ok_or("Bitcoin's peers are reached through a SOCKS5 proxy alone, such as Tor's, and the wallet's proxy is not one")?
        .trim_end_matches('/');
    let address = address.replacen("localhost:", "127.0.0.1:", 1);

    address.parse().map(Some).map_err(|_| format!("the proxy {setting} is not an IP address and port"))
}

/// A peer from Settings: host:port, or an IP address on the network's own port.
fn peer(text: &str, net: Net) -> Option<TrustedPeer> {
    let text = text.trim();

    if text.is_empty() {
        return None;
    }

    if let Ok(address) = text.parse::<SocketAddr>() {
        return Some(TrustedPeer::from_socket_addr(address));
    }

    if let Ok(ip) = text.parse::<std::net::IpAddr>() {
        return Some(TrustedPeer::from_ip(ip));
    }

    let port = match text.rsplit_once(':').and_then(|(_, port)| port.parse::<u16>().ok()) {
        Some(port) => port,
        None => match net {
            Net::Main => 8333,
            Net::Testnet4 => 48333,
            Net::Signet => 38333,
            Net::Regtest => 18444,
        },
    };
    let host = text.rsplit_once(':').map_or(text, |(host, _)| host);

    Some(TrustedPeer::from_hostname(host, port))
}

/// Takes a block's transactions in: outputs paying a watched script become coins, and inputs
/// spending a coin mark it spent, with their witness.
pub fn scan(seen: &mut Seen, block: &Block, height: u32, watched: &BTreeSet<Vec<u8>>) -> bool {
    use bitcoin::hashes::Hash;

    let mut found = false;
    // The block's transaction ids, in Bitcoin's own byte order, once a coin needs its path.
    let mut ids: Option<Vec<[u8; 32]>> = None;

    for (index, tx) in block.txdata.iter().enumerate() {
        let txid = tx.compute_txid().to_string();

        for input in &tx.input {
            let (spent_txid, spent_vout) = (input.previous_output.txid.to_string(), input.previous_output.vout);

            if let Some(coin) = seen.coins.iter_mut().find(|coin| coin.txid == spent_txid && coin.vout == spent_vout) {
                coin.spent = Some(Spent { txid: txid.clone(), height, witness: input.witness.iter().map(hex).collect() });
                found = true;
            }
        }

        for (vout, output) in tx.output.iter().enumerate() {
            let script = output.script_pubkey.as_bytes();

            if watched.contains(script) && !seen.coins.iter().any(|coin| coin.txid == txid && coin.vout == vout as u32) {
                let ids = ids.get_or_insert_with(|| block.txdata.iter().map(|tx| tx.compute_txid().to_byte_array()).collect());
                let mut base = tx.clone();
                base.input.iter_mut().for_each(|input| input.witness.clear());
                let proof = CoinProof {
                    block: hex(&block.block_hash().to_byte_array()),
                    raw: hex(&bitcoin::consensus::serialize(&base)),
                    index: index as u32,
                    path: chainv4::crypto::bitcoin::merkle_path(ids, index).iter().map(|sibling| hex(sibling)).collect(),
                };
                seen.coins.push(Coin { txid: txid.clone(), vout: vout as u32, value: output.value.to_sat(), script: hex(script), height, spent: None, proof: Some(proof) });
                found = true;
            }
        }
    }

    found
}

/// Forgets what blocks from a height on said: a reorganization took them away.
pub fn undo(seen: &mut Seen, from: u32) {
    seen.coins.retain(|coin| coin.height < from);

    for coin in &mut seen.coins {
        if coin.spent.as_ref().is_some_and(|spent| spent.height >= from) {
            coin.spent = None;
        }
    }
}

/// The secret an HTLC's claim showed, among the spends of its coins.
pub fn secret_in(seen: &Seen, script: &[u8], hash: &[u8; 32]) -> Option<[u8; 32]> {
    let script = hex(script);

    seen.coins
        .iter()
        .filter(|coin| coin.script == script)
        .filter_map(|coin| coin.spent.as_ref())
        .flat_map(|spent| &spent.witness)
        .filter_map(|item| chainv4::structs::hex::from_hex(&format!("0x{item}")))
        .filter_map(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .find(|secret| sha256(secret) == *hash)
}

async fn run(
    net: Net,
    shared: Arc<Mutex<Shared>>,
    proxy: Option<SocketAddr>,
    trusted: Option<String>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    wake: egui::Context,
) {
    // From where it last got to, or the checkpoint.
    let start = {
        let seen = &lock(&shared).seen;

        match seen.hash.parse::<BlockHash>() {
            Ok(hash) if seen.height > 0 => HashCheckpoint::new(seen.height, hash),
            _ => net.checkpoint(),
        }
    };
    let mut builder = Builder::new(net.network()).chain_state(ChainState::Checkpoint(start)).required_peers(if net == Net::Regtest { 1 } else { 2 });

    if let Some(peer) = trusted.as_deref().and_then(|text| peer(text, net)) {
        builder = builder.add_peer(peer);
    }

    if net == Net::Regtest {
        builder = builder.whitelist_only();
    }

    if let Some(proxy) = proxy {
        builder = builder.socks5_proxy(Socks5Proxy::new(proxy));
    }

    let (node, client) = builder.build();
    let running = shared.clone();
    tokio::spawn(async move {
        if let Err(err) = node.run().await {
            lock(&running).status.stopped = Some(err.to_string());
        }
    });
    let Client { requester, mut info_rx, mut warn_rx, mut event_rx } = client;
    // Once a block could not be fetched, where the next run begins stays before it.
    let mut held = false;

    loop {
        tokio::select! {
            _ = &mut stop => {
                let _ = requester.shutdown();
                return;
            }
            event = event_rx.recv() => match event {
                Some(Event::IndexedFilter(filter)) => {
                    let (height, hash) = (filter.height(), filter.block_hash());
                    let watched: Vec<ScriptBuf> = lock(&shared).watched.iter().map(|script| ScriptBuf::from_bytes(script.clone())).collect();

                    if filter.contains_any(watched.iter()) {
                        let block = match requester.request_block(hash) {
                            Ok(asked) => asked.await.ok().and_then(Result::ok),
                            Err(_) => None,
                        };

                        match block {
                            Some(indexed) => {
                                let mut shared = lock(&shared);
                                let watched = shared.watched.clone();
                                scan(&mut shared.seen, &indexed.block, indexed.height, &watched);
                            }
                            None => {
                                // Without the block, what it holds is not known: the next run
                                // begins before it, and asks again.
                                lock(&shared).status.warning = Some(format!("block {height} could not be fetched; it is asked again on the next start"));
                                held = true;
                            }
                        }
                    }

                    let mut shared = lock(&shared);
                    shared.status.filters = height;

                    if !held {
                        (shared.seen.height, shared.seen.hash) = (height, hash.to_string());
                    }
                }
                Some(Event::FiltersSynced(update)) => {
                    let mut shared = lock(&shared);
                    shared.status.synced = true;
                    shared.status.headers = shared.status.headers.max(update.tip().height);
                }
                Some(Event::ChainUpdate(BlockHeaderChanges::Connected(header))) => {
                    let mut shared = lock(&shared);
                    shared.status.headers = shared.status.headers.max(header.height);
                    shared.status.synced = false;
                }
                Some(Event::ChainUpdate(BlockHeaderChanges::Reorganized { accepted, reorganized })) => {
                    if let Some(from) = reorganized.iter().map(|header| header.height).min() {
                        let mut shared = lock(&shared);
                        undo(&mut shared.seen, from);

                        // Scanned again from the block they forked from.
                        if let Some(first) = accepted.iter().min_by_key(|header| header.height) {
                            (shared.seen.height, shared.seen.hash) = (from.saturating_sub(1), first.header.prev_blockhash.to_string());
                        }
                    }
                }
                Some(_) => {}
                None => return,
            },
            info = info_rx.recv() => match info {
                Some(Info::NewChainHeight(height)) => lock(&shared).status.headers = height,
                Some(Info::FilterAdded(height)) => lock(&shared).status.filters = height,
                Some(Info::ConnectionsMet) => lock(&shared).status.connected = true,
                Some(_) => {}
                None => return,
            },
            warning = warn_rx.recv() => {
                if let Some(warning) = warning {
                    lock(&shared).status.warning = Some(warning.to_string());
                }
            }
            command = commands.recv() => match command {
                Some(Command::Broadcast(id, raw)) => {
                    let result = match bitcoin::consensus::deserialize::<Transaction>(&raw) {
                        Ok(tx) => {
                            let txid = tx.compute_txid().to_string();
                            requester.submit_package(tx).await.map(|_| txid).map_err(|err| err.to_string())
                        }
                        Err(err) => Err(format!("not a transaction: {err}")),
                    };
                    lock(&shared).broadcast.push((id, result));
                }
                None => return,
            },
        }

        wake.request_repaint();
    }
}

#[cfg(test)]
mod tests;
