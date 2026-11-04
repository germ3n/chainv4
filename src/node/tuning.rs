//! What an operator may tune of a node beyond its plain flags: every limit and cadence the code
//! once fixed, in one table, each with its default, its bounds and what it governs. A few that
//! operators reach for, such as --ban-score and --http-rate, are flags of their own; the rest are
//! keys of --tune, KEY=VALUE, grouped by what they tune: sync., gossip., dial., peers., node.,
//! mining., http., range., rpc., filter., finality. and chain. None of it is consensus. Each
//! changes how much a node takes on, how fast, and with how much memory, never which blocks are
//! valid or what state they make, so each node picks its own, and an unset one is what every node
//! did before.

use {
    r#macro::document,
    std::collections::BTreeMap,
    std::time::Duration,
};

#[document(summary = "What a tuned value counts.", category = "Node", see_also = "Knob")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    #[document(summary = "A whole number of things.")]
    Count,
    #[document(summary = "A span of time, written 50ms, 15s, 2m, 1h or 1d, kept in milliseconds.")]
    Ms,
    #[document(summary = "A size, written in bytes or with KiB or MiB after it, kept in bytes.")]
    Bytes,
}

#[document(
    summary = "One value an operator may tune: its key, the flag of its own if it has one, what it counts, its default as written, the least and most it may be, and whether it may be left unset.",
    category = "Node",
    see_also = "KNOBS, Tuning",
)]
#[derive(Clone, Copy, Debug)]
pub struct Knob {
    #[document(summary = "Its key, as --tune takes it and GET /node's limits name it.")]
    pub key: &'static str,
    #[document(summary = "The flag that sets it, when it has one of its own; --tune takes the rest.")]
    pub flag: Option<&'static str>,
    #[document(summary = "What it counts.")]
    pub unit: Unit,
    #[document(summary = "Its default, as written, or none for one unset by default.")]
    pub default: &'static str,
    #[document(summary = "The least it may be, in its unit: below it the node would break, or do nothing.")]
    pub least: u64,
    #[document(summary = "The most it may be, in its unit: past it, a protocol cap, the codec's cap or the machine's memory.")]
    pub most: u64,
}

const fn count(key: &'static str, default: &'static str, least: u64, most: u64) -> Knob {
    Knob { key, flag: None, unit: Unit::Count, default, least, most }
}

const fn ms(key: &'static str, default: &'static str, least: u64, most: u64) -> Knob {
    Knob { key, flag: None, unit: Unit::Ms, default, least, most }
}

const fn flagged(knob: Knob, flag: &'static str) -> Knob {
    Knob { flag: Some(flag), ..knob }
}

const SECOND: u64 = 1_000;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const MIB: u64 = 1024 * 1024;

#[document(
    summary = "Every value an operator may tune, by key. Those with a flag of their own are set by it; the rest by --tune KEY=VALUE, repeated or comma-separated. Each default is what every node did before it could be tuned.",
    category = "Node",
    see_also = "Knob, Tuning",
)]
pub const KNOBS: &[Knob] = &[
    // Peers, bans and the gossip backlog.
    flagged(count("net.max-peers-in", "none", 1, 10_000), "--max-peers-in"),
    flagged(count("ban.score", "100", 10, 1_000_000), "--ban-score"),
    flagged(ms("ban.window", "10m", SECOND, 7 * DAY), "--ban-window"),
    flagged(count("backlog.strikes", "10", 1, 10_000), "--max-strikes"),
    flagged(ms("backlog.strike-window", "60s", SECOND, DAY), "--strike-window"),
    flagged(ms("tor.timeout", "90s", 5 * SECOND, 10 * MINUTE), "--tor-timeout"),
    // What the node serves syncing peers at most: never more than peers ask for.
    count("sync.blocks", "128", 1, crate::net::MAX_SYNC_BLOCKS as u64),
    count("sync.justifications", "64", 1, crate::net::MAX_SYNC_JUSTIFICATIONS as u64),
    count("sync.headers", "1024", 1, crate::net::MAX_SYNC_HEADERS as u64),
    count("sync.entries", "512", 1, crate::net::MAX_SYNC_ENTRIES as u64),
    // Gossip batching: peers refuse a batch past TX_BATCH, so it may only be smaller.
    count("gossip.batch", "256", 1, crate::net::TX_BATCH as u64),
    ms("gossip.wait", "50ms", 1, 10 * SECOND),
    // Dialling.
    ms("dial.redial", "15s", SECOND, HOUR),
    ms("dial.bootstrap", "120s", 10 * SECOND, DAY),
    count("dial.at-once", "3", 1, 64),
    ms("dial.retry", "300s", SECOND, DAY),
    count("peers.remembered", "64", 1, 10_000),
    // The node loop, and snapshots.
    count("node.event-budget", "256", 16, 65_536),
    flagged(count("snapshot.depth", "16", 1, 100_000), "--snapshot-depth"),
    flagged(ms("updates.every", "1h", MINUTE, 7 * DAY), "--update-every"),
    // The mining port.
    count("mining.connections", "256", 1, 65_536),
    count("mining.per-ip", "32", 1, 65_536),
    count("mining.devices", "64", 1, 1_024),
    ms("mining.handshake", "10s", SECOND, 5 * MINUTE),
    ms("mining.template", "2s", 100, MINUTE),
    count("mining.solo", "16", 1, 1_024),
    // The HTTP API.
    flagged(count("http.rate", "0", 0, 100_000), "--http-rate"),
    flagged(count("http.max-page", "none", 10, 10_000), "--api-max-page"),
    flagged(count("http.suggest-multiplier", "2", 1, 100), "--suggest-multiplier"),
    count("http.range-per-ip", "2", 1, 64),
    ms("http.range-wait", "2s", 100, MINUTE),
    count("http.relays-per-ip", "32", 1, 4_096),
    count("range.blocks", "100", 1, 10_000),
    Knob { key: "range.bytes", flag: None, unit: Unit::Bytes, default: "8MiB", least: MIB, most: 32 * MIB },
    count("range.entries", "4096", 1, 1_000_000),
    ms("range.time", "1s", 100, MINUTE),
    count("rpc.log-blocks", "10000", 1, 1_000_000),
    count("rpc.log-addresses", "1000", 1, 100_000),
    count("rpc.log-topics", "1000", 1, 100_000),
    count("rpc.batch", "100", 1, 10_000),
    count("rpc.fee-history", "1024", 1, 100_000),
    count("filter.scan", "1000", 1, 1_000_000),
    // What the chain remembers, and its engines.
    count("finality.pending", "4096", 1, 1_000_000),
    count("finality.evidence", "64", 1, 100_000),
    count("chain.bad-blocks", "1024", 1, 1_000_000),
    flagged(count("lua.states", "16", 1, 1_024), "--lua-states"),
    flagged(count("lua.builders", "2", 1, 64), "--lua-builders"),
    flagged(count("wasm.cache-mb", "32", 1, 4_096), "--wasm-cache-mb"),
    flagged(count("wasm.cache-modules", "1024", 1, 1_000_000), "--wasm-cache-modules"),
];

#[document(
    summary = "The knob a key names.",
    category = "Node",
    returns = "The knob, or why there is none, naming the keys --tune takes.",
    see_also = "KNOBS",
)]
pub fn knob(key: &str) -> Result<&'static Knob, String> {
    KNOBS.iter().find(|knob| knob.key == key).ok_or_else(|| {
        let keys: Vec<_> = KNOBS.iter().filter(|knob| knob.flag.is_none()).map(|knob| knob.key).collect();

        format!("{key} is nothing --tune sets: it takes {}", keys.join(", "))
    })
}

impl Knob {
    #[document(
        parent = "Knob",
        summary = "Reads a value as this knob counts it, and holds it to its bounds: a count, a time such as 50ms or 15s, or a size such as 8MiB.",
        returns = "The value in its unit, milliseconds for a time and bytes for a size; or why it will not do.",
    )]
    pub fn read(&self, text: &str) -> Result<u64, String> {
        let text = text.trim();
        let value = match self.unit {
            Unit::Count => text.parse::<u64>().map_err(|_| format!("{}: {text} is not a whole number", self.key))?,
            Unit::Ms => duration_ms(text).map_err(|why| format!("{}: {why}", self.key))?,
            Unit::Bytes => bytes(text).map_err(|why| format!("{}: {why}", self.key))?,
        };

        match value < self.least || value > self.most {
            true => Err(format!("{}: {text} is out of bounds: {} to {}", self.key, self.show(self.least), self.show(self.most))),
            false => Ok(value),
        }
    }

    #[document(parent = "Knob", summary = "Its default in its unit, or None for one unset by default.")]
    pub fn default_value(&self) -> Option<u64> {
        match self.default {
            "none" => None,
            text => Some(self.read(text).expect("every default is within its own bounds")),
        }
    }

    /// A value in its unit as the knob is written.
    fn show(&self, value: u64) -> String {
        match self.unit {
            Unit::Count => value.to_string(),
            Unit::Ms if !value.is_multiple_of(SECOND) => format!("{value}ms"),
            Unit::Ms => format!("{}s", value / SECOND),
            Unit::Bytes if value.is_multiple_of(MIB) => format!("{}MiB", value / MIB),
            Unit::Bytes => value.to_string(),
        }
    }
}

/// A time as the knobs take it: 50ms, 15s, 2m, 1h or 1d.
fn duration_ms(text: &str) -> Result<u64, String> {
    let bad = || format!("{text} is not a span of time, such as 50ms, 15s, 2m or 1h");
    let split = text.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (count, unit) = text.split_at(split);
    let count: u64 = count.parse().map_err(|_| bad())?;
    let unit = match unit {
        "ms" => 1,
        "s" => SECOND,
        "m" => MINUTE,
        "h" => HOUR,
        "d" => DAY,
        _ => return Err(bad()),
    };

    count.checked_mul(unit).ok_or_else(bad)
}

/// A size as the knobs take it: bytes, or KiB or MiB.
fn bytes(text: &str) -> Result<u64, String> {
    let bad = || format!("{text} is not a size, such as 8MiB, 512KiB or 1048576");
    let split = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let (count, unit) = text.split_at(split);
    let count: u64 = count.parse().map_err(|_| bad())?;
    let unit = match unit {
        "" => 1,
        "KiB" => 1024,
        "MiB" => MIB,
        _ => return Err(bad()),
    };

    count.checked_mul(unit).ok_or_else(bad)
}

#[document(
    summary = "Reads one KEY=VALUE as --tune takes it: a key of KNOBS without a flag of its own, and a value within its bounds.",
    category = "Node",
    returns = "The key and its value in its unit, or why it will not do.",
    see_also = "Tuning",
)]
pub fn setting(text: &str) -> Result<(&'static str, u64), String> {
    let (key, value) = text.split_once('=').ok_or_else(|| format!("{text} is not KEY=VALUE, such as sync.headers=512"))?;
    let knob = knob(key.trim())?;

    if let Some(flag) = knob.flag {
        return Err(format!("{} has a flag of its own: give {flag} instead", knob.key));
    }

    Ok((knob.key, knob.read(value)?))
}

#[document(
    summary = "A clap value parser for a knob's own flag: it reads the value as the knob counts it and holds it to the knob's bounds, so the flag and --tune never disagree.",
    category = "Node",
    params = { key = "The knob's key." },
    see_also = "Knob::read",
)]
pub fn flag(key: &'static str) -> impl Fn(&str) -> Result<u64, String> + Clone + Send + Sync + 'static {
    move |text| knob(key).and_then(|knob| knob.read(text))
}

#[document(
    summary = "A knob's default as written, for a flag's default_value.",
    category = "Node",
    params = { key = "The knob's key." },
)]
pub fn default_text(key: &str) -> &'static str {
    knob(key).map_or("", |knob| knob.default)
}

#[document(
    summary = "What an operator tuned: the value of every knob set, by key, the rest at their defaults. It hands each part of the node its piece: the network's NetTuning, the loop's NodeTuning, the mining port's PortLimits, the chain's ChainOptions and the API's ApiLimits; and GET /node shows every knob's value.",
    category = "Node",
    see_also = "KNOBS, setting",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tuning {
    set: BTreeMap<&'static str, u64>,
}

impl Tuning {
    #[document(parent = "Tuning", summary = "Everything at its default.")]
    pub fn new() -> Tuning {
        Tuning::default()
    }

    #[document(
        parent = "Tuning",
        summary = "Sets a knob, as its flag or --tune gave it, already read and bounded.",
        params = { key = "The knob's key, which must be in KNOBS.", value = "Its value in its unit." },
    )]
    pub fn set(&mut self, key: &'static str, value: u64) -> &mut Tuning {
        debug_assert!(knob(key).is_ok(), "{key} is a knob");
        self.set.insert(key, value);
        self
    }

    #[document(parent = "Tuning", summary = "Sets every KEY=VALUE --tune gave, a later one of a key winning.")]
    pub fn with(mut self, settings: &[(&'static str, u64)]) -> Tuning {
        for (key, value) in settings {
            self.set.insert(key, *value);
        }

        self
    }

    #[document(parent = "Tuning", summary = "A knob's value: what was set, or its default; None for one unset by default and not set.")]
    pub fn value(&self, key: &str) -> Option<u64> {
        match self.set.get(key) {
            Some(value) => Some(*value),
            None => knob(key).ok().and_then(Knob::default_value),
        }
    }

    /// A knob that always has a value.
    fn get(&self, key: &str) -> u64 {
        self.value(key).expect("a knob with a default")
    }

    fn count(&self, key: &str) -> usize {
        usize::try_from(self.get(key)).unwrap_or(usize::MAX)
    }

    fn time(&self, key: &str) -> Duration {
        Duration::from_millis(self.get(key))
    }

    #[document(parent = "Tuning", summary = "Every knob and its value as it stands, by key, None for one unset: what GET /node's limits show, times in milliseconds and sizes in bytes.")]
    pub fn values(&self) -> Vec<(&'static str, Option<u64>)> {
        KNOBS.iter().map(|knob| (knob.key, self.value(knob.key))).collect()
    }

    #[document(parent = "Tuning", summary = "The knobs set to other than their defaults, for the node to say at its start.")]
    pub fn changed(&self) -> Vec<(&'static str, u64)> {
        self.set.iter().filter(|(key, value)| knob(key).ok().and_then(Knob::default_value) != Some(**value)).map(|(key, value)| (*key, *value)).collect()
    }

    #[document(parent = "Tuning", summary = "The network's part: how many may dial in, gossip's batches and dialling.")]
    pub fn net(&self) -> crate::net::NetTuning {
        crate::net::NetTuning {
            max_peers_in: self.value("net.max-peers-in").map(|most| u32::try_from(most).unwrap_or(u32::MAX)),
            tx_batch: self.count("gossip.batch"),
            tx_batch_wait: self.time("gossip.wait"),
            redial_every: self.time("dial.redial"),
            bootstrap_every: self.time("dial.bootstrap"),
            dials_at_once: self.count("dial.at-once"),
            retry_after: self.time("dial.retry"),
        }
    }

    #[document(parent = "Tuning", summary = "How peers are banned, beside how long and whether by IP, which have flags of their own already.")]
    pub fn bans(&self, ban_for: Duration, ips: bool) -> crate::net::BanLimits {
        crate::net::BanLimits { ban_for, ips, score: u32::try_from(self.get("ban.score")).unwrap_or(u32::MAX), window: self.time("ban.window") }
    }

    #[document(parent = "Tuning", summary = "The gossip backlog's strike rules, beside its sizes, which have flags of their own already.")]
    pub fn backlog(&self, total: usize, per_peer: usize) -> crate::net::BacklogLimits {
        crate::net::BacklogLimits {
            total,
            per_peer,
            strikes: u32::try_from(self.get("backlog.strikes")).unwrap_or(u32::MAX),
            strike_window: self.time("backlog.strike-window"),
        }
    }

    #[document(parent = "Tuning", summary = "How long a dial through Tor may take.")]
    pub fn tor_timeout(&self) -> Duration {
        self.time("tor.timeout")
    }

    #[document(parent = "Tuning", summary = "The node loop's part: what it serves syncing peers, events a pass, mining's templates and payouts mining alone, peers remembered and the snapshot's depth.")]
    pub fn node(&self) -> super::NodeTuning {
        super::NodeTuning {
            sync_blocks: self.get("sync.blocks") as u32,
            sync_justifications: self.get("sync.justifications") as u32,
            sync_headers: self.get("sync.headers") as u32,
            sync_entries: self.count("sync.entries"),
            event_budget: self.count("node.event-budget"),
            template_ms: self.get("mining.template"),
            most_solo: self.count("mining.solo"),
            remembered: self.count("peers.remembered"),
            snapshot_depth: self.get("snapshot.depth"),
        }
    }

    #[document(parent = "Tuning", summary = "How often to look for releases, in milliseconds.")]
    pub fn update_every_ms(&self) -> u64 {
        self.get("updates.every")
    }

    #[document(parent = "Tuning", summary = "The mining port's part.")]
    pub fn port(&self) -> crate::mining::pool::PortLimits {
        crate::mining::pool::PortLimits {
            connections: self.count("mining.connections"),
            per_ip: self.count("mining.per-ip"),
            handshake: self.time("mining.handshake"),
            devices: self.count("mining.devices"),
            solo: self.count("mining.solo"),
        }
    }

    #[document(parent = "Tuning", summary = "The engines' pools.")]
    pub fn pools(&self) -> crate::vm::EnginePools {
        crate::vm::EnginePools {
            lua_states: self.count("lua.states"),
            lua_builders: self.count("lua.builders"),
            wasm_bytes: self.count("wasm.cache-mb").saturating_mul(1024 * 1024),
            wasm_modules: self.count("wasm.cache-modules"),
        }
    }

    #[document(parent = "Tuning", summary = "The chain's part: the engines' pools, and the bad blocks, waiting votes and evidence it remembers.")]
    pub fn chain(&self) -> crate::chain::ChainOptions {
        crate::chain::ChainOptions {
            pools: self.pools(),
            bad_blocks: self.count("chain.bad-blocks"),
            votes: crate::chain::VoteBounds { pending: self.count("finality.pending"), evidence: self.count("finality.evidence") },
        }
    }

    #[cfg(feature = "http")]
    #[document(parent = "Tuning", summary = "The API's part, beside --http-per-ip.")]
    pub fn api(&self) -> crate::api::ApiLimits {
        crate::api::ApiLimits {
            rate: u32::try_from(self.get("http.rate")).unwrap_or(u32::MAX),
            max_page: self.value("http.max-page"),
            range_per_ip: self.count("http.range-per-ip"),
            range_wait: self.time("http.range-wait"),
            relays_per_ip: self.count("http.relays-per-ip"),
            range: crate::chain::RangeLimits {
                blocks: self.get("range.blocks"),
                bytes: self.get("range.bytes"),
                entries: u32::try_from(self.get("range.entries")).unwrap_or(u32::MAX),
                time: self.time("range.time"),
            },
            rpc: crate::api::RpcLimits {
                log_blocks: self.get("rpc.log-blocks"),
                log_addresses: self.count("rpc.log-addresses"),
                log_topics: self.count("rpc.log-topics"),
                batch: self.count("rpc.batch"),
                fee_history: self.get("rpc.fee-history"),
            },
            filter_scan: self.count("filter.scan"),
            suggest: self.get("http.suggest-multiplier"),
        }
    }
}

#[document(
    summary = "Something counted in blocks that an operator may give as a time instead: a whole number is blocks, and a time, such as 14h or 3m, is turned into blocks at the chain's block time as the node starts. What --state-history and --underpriced-for take.",
    category = "Node",
    see_also = "Span::blocks",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    #[document(summary = "So many blocks.")]
    Blocks(u64),
    #[document(summary = "So many milliseconds, in blocks at the chain's block time.")]
    Ms(u64),
}

impl std::str::FromStr for Span {
    type Err = String;

    fn from_str(text: &str) -> Result<Span, String> {
        let text = text.trim();

        match text.parse::<u64>() {
            Ok(blocks) => Ok(Span::Blocks(blocks)),
            Err(_) => duration_ms(text).map(Span::Ms).map_err(|why| format!("{why}; or a whole number of blocks")),
        }
    }
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Span::Blocks(blocks) => write!(f, "{blocks}"),
            Span::Ms(ms) => write!(f, "{ms}ms"),
        }
    }
}

impl Span {
    #[document(
        parent = "Span",
        summary = "How many blocks it is: so many, or the time over the block time rounded up, at least one.",
        params = { block_ms = "Milliseconds between blocks, as the chain's consensus aims for now." },
    )]
    pub fn blocks(self, block_ms: u64) -> u64 {
        match self {
            Span::Blocks(blocks) => blocks,
            Span::Ms(ms) => ms.div_ceil(block_ms.max(1)).max(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_reads_within_its_own_bounds_and_matches_what_the_code_used_before() {
        for knob in KNOBS {
            if knob.default != "none" {
                assert!(knob.read(knob.default).is_ok(), "{} defaults within its bounds", knob.key);
            }
        }

        let tuning = Tuning::new();
        assert_eq!(tuning.net(), crate::net::NetTuning::DEFAULT);
        assert_eq!(tuning.node(), super::super::NodeTuning::DEFAULT);
        assert_eq!(tuning.port(), crate::mining::pool::PortLimits::DEFAULT);
        assert_eq!(tuning.chain(), crate::chain::ChainOptions::DEFAULT);
        assert_eq!(tuning.bans(crate::net::DEFAULT_BAN, false), crate::net::BanLimits::default());
        assert_eq!(tuning.backlog(10_000, 1_000), crate::net::BacklogLimits::default());
        assert_eq!(tuning.tor_timeout(), crate::net::tor::TOR_TIMEOUT);
        #[cfg(feature = "updates")]
        assert_eq!(tuning.update_every_ms(), super::super::CHECK_EVERY_MS);
        #[cfg(feature = "http")]
        assert_eq!(tuning.api(), crate::api::ApiLimits::DEFAULT);
        assert!(tuning.changed().is_empty());
    }

    #[test]
    fn tune_takes_keys_within_their_bounds_in_their_units() {
        assert_eq!(setting("sync.headers=512"), Ok(("sync.headers", 512)));
        assert_eq!(setting("gossip.wait=20ms"), Ok(("gossip.wait", 20)));
        assert_eq!(setting("dial.redial=1m"), Ok(("dial.redial", 60_000)));
        assert_eq!(setting("range.bytes=16MiB"), Ok(("range.bytes", 16 * 1024 * 1024)));

        // Past what peers ask for, or take, and below what does anything.
        assert!(setting("sync.blocks=129").unwrap_err().contains("out of bounds"));
        assert!(setting("gossip.batch=257").unwrap_err().contains("out of bounds"), "peers refuse a bigger batch");
        assert!(setting("sync.entries=0").unwrap_err().contains("out of bounds"));
        assert!(setting("range.bytes=64MiB").unwrap_err().contains("out of bounds"));
        assert!(setting("mining.template=10ms").unwrap_err().contains("out of bounds"));
        assert!(setting("node.event-budget=1").unwrap_err().contains("out of bounds"));

        // Not a key, not KEY=VALUE, not its unit, or a key with a flag of its own.
        assert!(setting("sync.everything=1").unwrap_err().contains("--tune sets"));
        assert!(setting("sync.headers").unwrap_err().contains("KEY=VALUE"));
        assert!(setting("gossip.wait=fast").unwrap_err().contains("span of time"));
        assert!(setting("ban.score=200").unwrap_err().contains("--ban-score"));
    }

    #[test]
    fn a_flag_reads_and_bounds_its_knob_as_tune_would() {
        let score = flag("ban.score");
        assert_eq!(score("200"), Ok(200));
        assert!(score("5").unwrap_err().contains("out of bounds"), "below the lightest fault every fault bans");

        let window = flag("ban.window");
        assert_eq!(window("30m"), Ok(30 * 60_000));
        assert!(window("0s").is_err());

        let peers = flag("net.max-peers-in");
        assert_eq!(peers("40"), Ok(40));
        assert!(peers("0").is_err(), "a node no peer may dial in to wants --max-peers-in left out, not 0");

        assert_eq!(flag("http.rate")("0"), Ok(0), "0 counts nothing");
        assert!(flag("http.suggest-multiplier")("0").is_err(), "an offer under the base price never lands");
        assert!(flag("lua.states")("0").is_err());
        assert!(flag("wasm.cache-mb")("5000").is_err());
        assert!(flag("tor.timeout")("1s").is_err());
        assert!(flag("updates.every")("10s").is_err());
    }

    #[test]
    fn what_is_tuned_reaches_each_part_of_the_node() {
        let mut tuning = Tuning::new().with(&[
            ("sync.headers", 256),
            ("gossip.batch", 64),
            ("dial.retry", 60_000),
            ("peers.remembered", 8),
            ("mining.solo", 4),
            ("mining.per-ip", 2),
            ("finality.pending", 100),
            ("chain.bad-blocks", 10),
            ("rpc.batch", 20),
            ("range.blocks", 10),
        ]);
        tuning.set("net.max-peers-in", 40).set("ban.score", 200).set("lua.states", 4).set("wasm.cache-mb", 64).set("snapshot.depth", 8).set("http.rate", 30);

        assert_eq!(tuning.net().max_peers_in, Some(40));
        assert_eq!((tuning.net().tx_batch, tuning.net().retry_after), (64, Duration::from_secs(60)));
        assert_eq!(tuning.bans(Duration::from_secs(60), true).score, 200);
        assert_eq!((tuning.node().sync_headers, tuning.node().remembered, tuning.node().most_solo, tuning.node().snapshot_depth), (256, 8, 4, 8));
        assert_eq!((tuning.port().per_ip, tuning.port().solo), (2, 4), "the port and the loop take the same payouts mining alone");
        assert_eq!((tuning.chain().pools.lua_states, tuning.chain().pools.wasm_bytes), (4, 64 * 1024 * 1024));
        assert_eq!((tuning.chain().bad_blocks, tuning.chain().votes.pending), (10, 100));
        #[cfg(feature = "http")]
        assert_eq!((tuning.api().rate, tuning.api().rpc.batch, tuning.api().range.blocks), (30, 20, 10));
        assert_eq!(tuning.changed().len(), 16);
        assert_eq!(tuning.values().iter().find(|(key, _)| *key == "http.max-page"), Some(&("http.max-page", None)), "unset by default");
    }

    #[test]
    fn a_span_is_blocks_or_a_time_in_blocks_at_the_block_time() {
        assert_eq!("10000".parse::<Span>(), Ok(Span::Blocks(10_000)));
        assert_eq!("14h".parse::<Span>(), Ok(Span::Ms(14 * 3_600_000)));
        assert!("soon".parse::<Span>().is_err());

        assert_eq!(Span::Blocks(32).blocks(5_000), 32);
        assert_eq!(Span::Ms(160_000).blocks(5_000), 32, "160 s of 5-second blocks");
        assert_eq!(Span::Ms(160_000).blocks(2_000), 80);
        assert_eq!(Span::Ms(1).blocks(5_000), 1, "never none");
    }
}
