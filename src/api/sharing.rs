//! What a node shares of itself with whoever asks its API: a preset, --report, with single fields
//! hidden or shown on top, --hide and --show. By default it keeps the addresses that place its
//! peers, its miners and itself on the internet, and the names its workers go by, which are host
//! names unless told otherwise.
//!
//! The operator sees everything: a request from this machine with nothing forwarding it, or one
//! bearing the report token, --report-token. What is hidden keeps its place in the answer, empty,
//! so apps from before still read it, and the answer's `hidden` names what was taken out.

use {
    axum::http::{HeaderMap, header},
    r#macro::document,
    std::net::SocketAddr,
    std::str::FromStr,
};

#[document(
    summary = "A part of what a node reports of itself that its operator can keep to itself, with --hide, or share, with --show.",
    category = "Node",
    see_also = "Sharing, ReportPreset",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportField {
    #[document(summary = "Where each peer connects from: GET /node's peers and /stream's peer_up.")]
    PeerAddresses,
    #[document(summary = "Each peer's id, and the peer a node syncs from.")]
    PeerIds,
    #[document(summary = "The peers altogether: GET /node's list, and /stream's peer_up and peer_down.")]
    Peers,
    #[document(summary = "Where the node itself listens: its own addresses, and the address its mining port is bound to.")]
    Listening,
    #[document(summary = "Where each mining worker connects from.")]
    WorkerAddresses,
    #[document(summary = "What each worker calls itself, by default its machine's host name: shown as worker 1, worker 2 and so on. In GET /pool/{address} too.")]
    WorkerNames,
    #[document(summary = "The address each worker's earnings go to.")]
    WorkerPayouts,
    #[document(summary = "The workers altogether: the lists in GET /node and GET /pool/{address}, and GET /mining's count.")]
    Workers,
    #[document(summary = "The GPUs and CPUs mining, the node's and its workers', by name.")]
    Devices,
    #[document(summary = "GET /node's mining section altogether.")]
    Mining,
    #[document(summary = "What the node counted since it started.")]
    Counters,
    #[document(summary = "The gossip and submissions waiting for the node, and its limits.")]
    Backlog,
    #[document(summary = "Whether, and from what height, the node is syncing.")]
    Syncing,
    #[document(summary = "When the node started, and so how long it has been up.")]
    Uptime,
    #[document(summary = "Whether the node produces blocks and whether it validates.")]
    Role,
    #[document(summary = "What its operator tuned: every value of KNOBS as it stands, GET /node's limits.")]
    Limits,
}

impl ReportField {
    #[document(parent = "ReportField", summary = "Every field.")]
    pub const ALL: [ReportField; 16] = [
        ReportField::PeerAddresses,
        ReportField::PeerIds,
        ReportField::Peers,
        ReportField::Listening,
        ReportField::WorkerAddresses,
        ReportField::WorkerNames,
        ReportField::WorkerPayouts,
        ReportField::Workers,
        ReportField::Devices,
        ReportField::Mining,
        ReportField::Counters,
        ReportField::Backlog,
        ReportField::Syncing,
        ReportField::Uptime,
        ReportField::Role,
        ReportField::Limits,
    ];

    #[document(parent = "ReportField", summary = "Its name, as --hide and --show take it and an answer's hidden lists it.")]
    pub fn name(self) -> &'static str {
        match self {
            ReportField::PeerAddresses => "peer-addresses",
            ReportField::PeerIds => "peer-ids",
            ReportField::Peers => "peers",
            ReportField::Listening => "listening",
            ReportField::WorkerAddresses => "worker-addresses",
            ReportField::WorkerNames => "worker-names",
            ReportField::WorkerPayouts => "worker-payouts",
            ReportField::Workers => "workers",
            ReportField::Devices => "devices",
            ReportField::Mining => "mining",
            ReportField::Counters => "counters",
            ReportField::Backlog => "backlog",
            ReportField::Syncing => "syncing",
            ReportField::Uptime => "uptime",
            ReportField::Role => "role",
            ReportField::Limits => "limits",
        }
    }

    fn bit(self) -> u32 {
        1 << self as u32
    }
}

impl FromStr for ReportField {
    type Err = String;

    fn from_str(name: &str) -> Result<ReportField, String> {
        ReportField::ALL.into_iter().find(|field| field.name() == name).ok_or_else(|| {
            let names: Vec<_> = ReportField::ALL.iter().map(|field| field.name()).collect();

            format!("{name} is no field of the node's report: it has {}", names.join(", "))
        })
    }
}

#[document(
    summary = "What a node shares to begin with, before --hide and --show.",
    category = "Node",
    see_also = "Sharing, ReportField",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportPreset {
    #[document(summary = "Everything.")]
    Full,
    #[document(summary = "All but the addresses of its peers, its workers and itself, and its workers' names: the default.")]
    #[default]
    Public,
    #[document(summary = "Its own state alone, how long it has been up, its role, its mempool and whether it syncs: no peers, no mining, no counters, no backlog, no limits.")]
    Minimal,
}

impl ReportPreset {
    fn hidden(self) -> &'static [ReportField] {
        const PUBLIC: [ReportField; 4] = [ReportField::PeerAddresses, ReportField::Listening, ReportField::WorkerAddresses, ReportField::WorkerNames];
        const MINIMAL: [ReportField; 9] = [ReportField::PeerAddresses, ReportField::Listening, ReportField::WorkerAddresses, ReportField::WorkerNames, ReportField::Peers, ReportField::Mining, ReportField::Counters, ReportField::Backlog, ReportField::Limits];

        match self {
            ReportPreset::Full => &[],
            ReportPreset::Public => &PUBLIC,
            ReportPreset::Minimal => &MINIMAL,
        }
    }
}

impl FromStr for ReportPreset {
    type Err = String;

    fn from_str(name: &str) -> Result<ReportPreset, String> {
        match name {
            "full" => Ok(ReportPreset::Full),
            "public" => Ok(ReportPreset::Public),
            "minimal" => Ok(ReportPreset::Minimal),
            _ => Err(format!("{name} is no report preset: it is full, public or minimal")),
        }
    }
}

#[document(
    summary = "Which fields of its report a node keeps to itself: a ReportPreset, with single fields hidden or shown on top.",
    category = "Node",
    see_also = "ReportField, ReportPreset, ApiOptions",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sharing {
    hidden: u32,
}

impl Default for Sharing {
    fn default() -> Sharing {
        Sharing::of(ReportPreset::default())
    }
}

impl Sharing {
    #[document(parent = "Sharing", summary = "What the operator sees: everything.")]
    pub const EVERYTHING: Sharing = Sharing { hidden: 0 };

    fn of(preset: ReportPreset) -> Sharing {
        Sharing { hidden: preset.hidden().iter().fold(0, |hidden, field| hidden | field.bit()) }
    }

    #[document(
        parent = "Sharing",
        summary = "A preset, with these fields hidden and these shown on top.",
        returns = "The sharing, or why not: a field named both to hide and to show.",
    )]
    pub fn new(preset: ReportPreset, hide: &[ReportField], show: &[ReportField]) -> Result<Sharing, String> {
        if let Some(both) = hide.iter().find(|field| show.contains(field)) {
            return Err(format!("{} is both hidden and shown: --hide or --show, not both", both.name()));
        }

        let hidden = hide.iter().fold(Sharing::of(preset).hidden, |hidden, field| hidden | field.bit());

        Ok(Sharing { hidden: show.iter().fold(hidden, |hidden, field| hidden & !field.bit()) })
    }

    #[document(parent = "Sharing", summary = "Whether it keeps this field to itself.")]
    pub fn hides(self, field: ReportField) -> bool {
        self.hidden & field.bit() != 0
    }

    #[document(parent = "Sharing", summary = "The names of the fields it keeps to itself, as an answer's hidden lists them.")]
    pub fn hidden_names(self) -> Vec<&'static str> {
        ReportField::ALL.into_iter().filter(|field| self.hides(*field)).map(ReportField::name).collect()
    }
}

/// Whether a request comes from this machine with nothing forwarding it. A proxy here says who it
/// forwards for in X-Forwarded-For, the last of which it wrote itself; only a TCP peer on this
/// machine counts, so nothing on the network gets in by writing that header. A proxy here that
/// does not say, as a Tor forward to 127.0.0.1 does not, passes everyone off as this machine.
fn from_this_machine(peer: SocketAddr, headers: &HeaderMap) -> bool {
    let forwarded = headers.get("x-forwarded-for").map(|value| value.to_str().ok().and_then(|value| value.rsplit(',').next()).and_then(|last| last.trim().parse::<std::net::IpAddr>().ok()));

    peer.ip().is_loopback()
        && match forwarded {
            None => true,
            Some(Some(ip)) => ip.is_loopback(),
            Some(None) => false,
        }
}

/// Whether two tokens are the same, in time that does not say how much of them is.
fn same_token(given: &[u8], token: &[u8]) -> bool {
    given.len() == token.len() && given.iter().zip(token).fold(0, |differ, (a, b)| differ | (a ^ b)) == 0
}

/// What this asker is shared, and whether it bore a token the node does not take, for an app to
/// say so: the operator, from this machine or with the token, sees everything.
pub(super) fn shared_with(sharing: Sharing, token: Option<&[u8]>, peer: SocketAddr, headers: &HeaderMap) -> (Sharing, bool) {
    let bearing = headers.get(header::AUTHORIZATION).and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("Bearer "));
    let taken = bearing.zip(token).is_some_and(|(given, token)| same_token(given.trim().as_bytes(), token));
    let refused = bearing.is_some() && !taken;

    match taken || from_this_machine(peer, headers) {
        true => (Sharing::EVERYTHING, refused),
        false => (sharing, refused),
    }
}

#[cfg(test)]
mod tests {
    use {super::*, axum::http::HeaderValue};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs.iter().map(|(name, value)| (axum::http::HeaderName::from_static(name), HeaderValue::from_str(value).unwrap())).collect()
    }

    #[test]
    fn a_preset_is_where_hide_and_show_start_from() {
        let public = Sharing::default();

        assert_eq!(public.hidden_names(), ["peer-addresses", "listening", "worker-addresses", "worker-names"]);
        assert!(!public.hides(ReportField::PeerIds) && !public.hides(ReportField::Counters), "ids, counters and hashrates stay");
        assert!(Sharing::of(ReportPreset::Full).hidden_names().is_empty());

        let minimal = Sharing::of(ReportPreset::Minimal);
        for field in [ReportField::Peers, ReportField::Mining, ReportField::Counters, ReportField::Backlog, ReportField::PeerAddresses] {
            assert!(minimal.hides(field), "{}", field.name());
        }
        for field in [ReportField::Uptime, ReportField::Role, ReportField::Syncing] {
            assert!(!minimal.hides(field), "{}", field.name());
        }

        let tweaked = Sharing::new(ReportPreset::Public, &[ReportField::Counters], &[ReportField::PeerAddresses]).unwrap();
        assert!(tweaked.hides(ReportField::Counters) && !tweaked.hides(ReportField::PeerAddresses) && tweaked.hides(ReportField::Listening));
        assert!(Sharing::new(ReportPreset::Public, &[ReportField::Peers], &[ReportField::Peers]).unwrap_err().contains("both hidden and shown"));
    }

    #[test]
    fn fields_and_presets_go_by_their_names() {
        for field in ReportField::ALL {
            assert_eq!(field.name().parse(), Ok(field));
        }

        assert!("ip".parse::<ReportField>().unwrap_err().contains("peer-addresses"), "says what there is");
        assert_eq!("minimal".parse(), Ok(ReportPreset::Minimal));
        assert!("private".parse::<ReportPreset>().is_err());
    }

    #[test]
    fn the_operator_sees_everything_from_this_machine_or_with_the_token() {
        let (local, lan, far) = ("127.0.0.1:5000".parse().unwrap(), "10.89.0.5:5000".parse().unwrap(), "203.0.113.7:5000".parse().unwrap());
        let public = Sharing::default();
        let token = Some(&b"0123456789abcdef0123456789abcdef"[..]);
        let sees = |peer, pairs: &[(&'static str, &str)]| shared_with(public, token, peer, &headers(pairs));

        assert_eq!(sees(local, &[]), (Sharing::EVERYTHING, false), "this machine");
        assert_eq!(sees("[::1]:5000".parse().unwrap(), &[]), (Sharing::EVERYTHING, false));
        assert_eq!(sees(far, &[]), (public, false));
        assert_eq!(sees(lan, &[]), (public, false), "the network is not this machine, as Docker's Tor is not");

        // nginx on this machine says who it forwards for, and that is who is asking.
        assert_eq!(sees(local, &[("x-forwarded-for", "203.0.113.7")]), (public, false));
        assert_eq!(sees(local, &[("x-forwarded-for", "127.0.0.1, 203.0.113.7")]), (public, false), "the last is the proxy's own");
        assert_eq!(sees(local, &[("x-forwarded-for", "not an address")]), (public, false));
        assert_eq!(sees(local, &[("x-forwarded-for", "127.0.0.1")]), (Sharing::EVERYTHING, false));

        // Nobody off this machine gets in by writing the header.
        assert_eq!(sees(far, &[("x-forwarded-for", "127.0.0.1")]), (public, false));
        assert_eq!(sees(lan, &[("x-forwarded-for", "127.0.0.1")]), (public, false));

        // The token, from anywhere; a wrong one shares no more, and is said to be wrong.
        assert_eq!(sees(far, &[("authorization", "Bearer 0123456789abcdef0123456789abcdef")]), (Sharing::EVERYTHING, false));
        assert_eq!(sees(far, &[("authorization", "Bearer 0123456789abcdef0123456789abcdee")]), (public, true));
        assert_eq!(sees(far, &[("authorization", "Bearer short")]), (public, true));
        assert_eq!(sees(far, &[("authorization", "Basic 0123456789abcdef0123456789abcdef")]), (public, false), "no bearer, no token");
        assert_eq!(shared_with(public, None, far, &headers(&[("authorization", "Bearer anything")])), (public, true), "a node with no token takes none");
    }
}
