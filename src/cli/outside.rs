//! The flags naming outside services the bridge, the anchorer, the swap CLI and the relay feeders
//! connect to (docs/bridges.md, Endpoints): each repeats, each URL with a login matched by order,
//! and all of them through --proxy.

use {
    super::CliResult,
    crate::remote::{Endpoint, Endpoints, Http, UreqHttp, beacon::Beacon},
    std::sync::Arc,
};

/// Bitcoin's sources: Esplora's and bitcoind's URLs.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct BitcoinArgs {
    /// An Esplora API, such as https://mempool.space/api; give it again for others to fall back
    /// to, in order.
    #[arg(long, global = true)]
    pub esplora: Vec<String>,
    /// A bitcoind's JSON-RPC, such as http://127.0.0.1:8332, asked after every --esplora fails; give
    /// it again for others. It needs no wallet; transactions already in a block are found by id with
    /// -txindex.
    #[arg(long, global = true)]
    pub bitcoind: Vec<String>,
    /// The login of each --bitcoind, by order: cookie:PATH (its .cookie, read again when bitcoind
    /// restarts), file:PATH (user:password, readable by its owner alone) or env:NAME.
    #[arg(long, global = true)]
    pub bitcoind_auth: Vec<String>,
}

/// Logins for --eth-rpc's URLs.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct EthAuthArgs {
    /// The login of each --eth-rpc, by order: file:PATH or env:NAME, holding user:password for basic
    /// auth or a token for a bearer header.
    #[arg(long, global = true)]
    pub eth_rpc_auth: Vec<String>,
}

/// An Ethereum beacon API's URLs.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct BeaconArgs {
    /// An Ethereum beacon API serving the light client routes, such as Lodestar's
    /// https://lodestar-sepolia.chainsafe.io; give it again for others to fall back to.
    #[arg(long, global = true)]
    pub beacon: Vec<String>,
    /// The login of each --beacon, by order, as --eth-rpc-auth takes them.
    #[arg(long, global = true)]
    pub beacon_auth: Vec<String>,
}

/// The proxy every outside connection goes through.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct ProxyArgs {
    /// A SOCKS5 proxy for every connection to Esplora, bitcoind, EVM chains and beacon APIs:
    /// socks5://host:port, or socks5h://host:port to resolve names there too, as Tor wants.
    #[arg(long, global = true)]
    pub proxy: Option<String>,
}

impl ProxyArgs {
    /// The HTTP client outside connections go through.
    pub fn http(&self) -> CliResult<Arc<dyn Http>> {
        Ok(Arc::new(UreqHttp::new(self.proxy.as_deref())?))
    }
}

#[cfg(feature = "bitcoin")]
impl BitcoinArgs {
    /// Whether any source is named.
    pub fn given(&self) -> bool {
        !self.esplora.is_empty() || !self.bitcoind.is_empty()
    }

    /// Bitcoin as these name it.
    pub fn source(&self, proxy: &ProxyArgs) -> CliResult<Option<crate::remote::bitcoin::Fallback>> {
        Ok(crate::remote::bitcoin::from_flags(proxy.http()?, &self.esplora, &self.bitcoind, &self.bitcoind_auth)?)
    }
}

impl BeaconArgs {
    /// The beacon API these name: refused for none.
    pub fn beacon(&self, proxy: &ProxyArgs) -> CliResult<Beacon> {
        if self.beacon.is_empty() {
            return Err("give --beacon: an Ethereum beacon API serving the light client routes, such as https://lodestar-sepolia.chainsafe.io".into());
        }

        Ok(Beacon::new(proxy.http()?, Endpoints::from_flags(&self.beacon, &self.beacon_auth, "beacon", false)?))
    }
}

impl EthAuthArgs {
    /// --eth-rpc's URLs with their logins, by order.
    pub fn endpoints(&self, urls: &[String]) -> CliResult<Vec<Endpoint>> {
        Ok(Endpoints::from_flags(urls, &self.eth_rpc_auth, "eth-rpc", false)?.list().to_vec())
    }
}
