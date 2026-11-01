//! The reverse proxies whose X-Forwarded-For the API believes, --trusted-proxy. Each is an address,
//! or a name for what has no fixed one: in Docker's stack nginx is another container, whose address
//! its next start may change, so the names are looked up again every so often. Until a name is found
//! its proxy counts as a client like any other, the same that happens with none named: visitors share
//! its limits, and nobody can spoof anything.

use std::{
    net::IpAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

/// How often names are looked up again: how long a proxy that came back on a new address stays a
/// plain client.
const LOOK_AGAIN: Duration = Duration::from_secs(10);

/// Each name looked up, with what it came to.
type Found = Vec<(String, Vec<IpAddr>)>;

#[derive(Clone, Default)]
pub(super) struct TrustedProxies {
    /// Those named by address.
    addresses: Arc<[IpAddr]>,
    /// Those named by a host's name.
    names: Arc<[String]>,
    /// What those names came to when last looked up. A name that stops resolving keeps its last.
    found: Arc<RwLock<Found>>,
}

impl TrustedProxies {
    /// From what --trusted-proxy was given: each an address if it reads as one, or else a name.
    pub(super) fn new(given: Vec<String>) -> TrustedProxies {
        let (addresses, names): (Vec<_>, Vec<_>) = given.into_iter().partition(|proxy| proxy.parse::<IpAddr>().is_ok());

        TrustedProxies {
            addresses: addresses.iter().filter_map(|address| address.parse().ok()).collect(),
            names: names.into(),
            found: Arc::default(),
        }
    }

    pub(super) fn contains(&self, ip: &IpAddr) -> bool {
        self.addresses.contains(ip) || self.found.read().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|(_, found)| found.contains(ip))
    }

    /// Whether there are names to look up.
    pub(super) fn named(&self) -> bool {
        !self.names.is_empty()
    }

    /// Looks every name up, saying when one's addresses change.
    async fn look_up(&self) {
        for name in self.names.iter() {
            let Ok(found) = tokio::net::lookup_host((name.as_str(), 0)).await else {
                continue;
            };
            let mut found: Vec<IpAddr> = found.map(|at| at.ip()).collect();
            found.sort();
            found.dedup();

            if found.is_empty() {
                continue;
            }

            let mut kept = self.found.write().unwrap_or_else(std::sync::PoisonError::into_inner);

            match kept.iter_mut().find(|(known, _)| known == name) {
                Some((_, addresses)) if *addresses == found => {}
                Some((_, addresses)) => {
                    log::info!("[http] the trusted proxy {name} is at {found:?} now");
                    *addresses = found;
                }
                None => {
                    log::info!("[http] the trusted proxy {name} is at {found:?}");
                    kept.push((name.clone(), found));
                }
            }
        }
    }

    /// Looks the names up for as long as the API runs.
    pub(super) async fn keep_looking(self) {
        loop {
            self.look_up().await;
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_proxy_named_by_address_is_trusted_from_the_start_and_no_other() {
        let proxies = TrustedProxies::new(vec!["172.18.0.3".to_string()]);

        assert!(proxies.contains(&ip("172.18.0.3")));
        assert!(!proxies.contains(&ip("172.18.0.4")));
        assert!(!proxies.named(), "nothing to look up");
        assert!(!TrustedProxies::default().contains(&ip("172.18.0.3")));
    }

    #[tokio::test]
    async fn a_proxy_named_by_host_is_trusted_at_what_it_resolves_to_and_only_once_it_does() {
        let proxies = TrustedProxies::new(vec!["localhost".to_string()]);
        let loopback = [ip("127.0.0.1"), ip("::1")];

        assert!(proxies.named());
        assert!(!loopback.iter().any(|at| proxies.contains(at)), "not before it is looked up");

        proxies.look_up().await;

        assert!(loopback.iter().any(|at| proxies.contains(at)));
        assert!(!proxies.contains(&ip("172.18.0.3")));
    }

    #[tokio::test]
    async fn a_name_that_stops_resolving_keeps_its_last_address_and_one_that_moves_loses_its_old() {
        let proxies = TrustedProxies::new(vec!["nothing-here.invalid".to_string()]);

        proxies.look_up().await;
        assert!(!proxies.contains(&ip("172.18.0.3")), "never found, never trusted");

        proxies.found.write().unwrap().push(("nothing-here.invalid".to_string(), vec![ip("172.18.0.3")]));
        proxies.look_up().await;
        assert!(proxies.contains(&ip("172.18.0.3")), "a lookup failing for a moment does not make visitors share limits");

        let moved = TrustedProxies::new(vec!["localhost".to_string()]);
        moved.found.write().unwrap().push(("localhost".to_string(), vec![ip("172.18.0.3")]));
        moved.look_up().await;
        assert!(!moved.contains(&ip("172.18.0.3")), "whatever has that address now is not the proxy");
    }
}
