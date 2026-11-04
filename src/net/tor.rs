//! Peer-to-peer over Tor. Nodes talk over QUIC, which is UDP and which Tor cannot carry, so a node
//! that is to be reached as an onion, or reach one, also speaks libp2p over TCP, with Noise and
//! Yamux, and dials through a Tor SOCKS5 proxy: the Docker stack's own tor, or Orbot's or Tor's on
//! a laptop. Three ways to run:
//!
//! - **Without a proxy,** as before: QUIC, and TCP for whatever `--listen` gives a TCP address.
//! - **With `--tor-proxy`,** `/onion3` addresses are dialed through Tor, everything else directly.
//! - **With `--tor-only` too,** no QUIC at all, and every TCP address, onion or not, is dialed
//!   through Tor: the node never shows its IP. It still listens on TCP, for the onion's
//!   connections, which tor brings to it from its own side.
//!
//! The node is reached as an onion by tor publishing its TCP port; its address, which tor makes,
//! comes to the node in a file (`--onion-file`), read until it is there, and told to peers.

use {
    libp2p::futures::future::BoxFuture,
    libp2p::core::muxing::StreamMuxerBox,
    libp2p::core::transport::{Boxed, DialOpts, ListenerId, TransportError, TransportEvent, upgrade::Version},
    libp2p::multiaddr::Protocol,
    libp2p::{Multiaddr, PeerId, Transport, noise, tcp, yamux},
    r#macro::document,
    std::path::PathBuf,
    std::pin::Pin,
    std::task::{Context, Poll},
    std::time::Duration,
    tokio::io::{AsyncReadExt, AsyncWriteExt},
};

#[document(summary = "How the node uses Tor for peer-to-peer: the SOCKS5 proxy it dials through, whether everything goes through it, where tor writes the node's onion, and how long a dial through it may take.", category = "Node", see_also = "NetConfig")]
#[derive(Clone, Debug)]
pub struct TorConfig {
    #[document(summary = "Tor's SOCKS5 proxy, socks5h://host:port: /onion3 peers are dialed through it. None dials no onions.")]
    pub proxy: Option<String>,
    #[document(summary = "Every dial through the proxy and no QUIC, so the node never shows its IP: it reaches onions, and TCP peers through Tor's exits.")]
    pub only: bool,
    #[document(summary = "A file holding the node's onion as a multiaddr, /onion3/…:PORT, which tor writes once it made it: read until it is there, then told to peers as the node's address.")]
    pub onion_file: Option<PathBuf>,
    #[document(summary = "How long Tor may take to reach a peer, building a circuit to an onion first: TOR_TIMEOUT unless --tor-timeout says otherwise.")]
    pub timeout: Duration,
}

impl Default for TorConfig {
    fn default() -> TorConfig {
        TorConfig { proxy: None, only: false, onion_file: None, timeout: TOR_TIMEOUT }
    }
}

#[document(summary = "How long Tor may take to reach a peer unless --tor-timeout says otherwise: 90 seconds, as it builds a circuit to an onion first.", category = "Node", see_also = "TorConfig::timeout")]
pub const TOR_TIMEOUT: Duration = Duration::from_secs(90);

#[document(summary = "Reads the node's onion address from the file tor writes: None until it is there, or while it holds no /onion3 multiaddr.", category = "Node", see_also = "TorConfig")]
pub fn read_onion(file: &std::path::Path) -> Option<Multiaddr> {
    let address: Multiaddr = std::fs::read_to_string(file).ok()?.trim().parse().ok()?;

    matches!(address.iter().next(), Some(Protocol::Onion3(_))).then_some(address)
}

#[document(summary = "The host of an onion's multiaddr, NAME.onion: None for an address that is no onion.", category = "Node", see_also = "read_onion")]
pub fn onion_host(address: &Multiaddr) -> Option<String> {
    let text = address.to_string();
    let name = text.strip_prefix("/onion3/")?.split(':').next()?;

    (!name.is_empty()).then(|| format!("{name}.onion"))
}

/// Where to dial an address through SOCKS5, the name as the proxy resolves it: an onion's, a DNS
/// name's, or an IP's as text. None for an address that is not TCP, such as QUIC's.
fn target(address: &Multiaddr, everything: bool) -> Option<(String, u16)> {
    let mut parts = address.iter();

    match (parts.next()?, parts.next()) {
        (Protocol::Onion3(onion), _) => {
            // /onion3/NAME:PORT, the name in base32 as Tor takes it.
            let text = Protocol::Onion3(onion.clone()).to_string();
            let name = text.strip_prefix("/onion3/")?.split(':').next()?.to_string();

            Some((format!("{name}.onion"), onion.port()))
        }
        _ if !everything => None,
        (Protocol::Ip4(ip), Some(Protocol::Tcp(port))) => Some((ip.to_string(), port)),
        (Protocol::Ip6(ip), Some(Protocol::Tcp(port))) => Some((ip.to_string(), port)),
        (Protocol::Dns(name) | Protocol::Dns4(name) | Protocol::Dns6(name), Some(Protocol::Tcp(port))) => Some((name.to_string(), port)),
        _ => None,
    }
}

/// A connection through a SOCKS5 proxy, which resolves the name: no authentication, as Tor's and
/// Orbot's take none, then CONNECT to the name.
async fn socks5(proxy: &str, host: &str, port: u16) -> std::io::Result<tokio::net::TcpStream> {
    use std::io::{Error, ErrorKind};

    let at = proxy.strip_prefix("socks5h://").or_else(|| proxy.strip_prefix("socks5://")).ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("the proxy {proxy} is not socks5h://host:port")))?;
    let name = u8::try_from(host.len()).map_err(|_| Error::new(ErrorKind::InvalidInput, "a name too long for SOCKS5"))?;
    let mut stream = tokio::net::TcpStream::connect(at).await?;
    stream.write_all(&[5, 1, 0]).await?;
    let mut chosen = [0; 2];
    stream.read_exact(&mut chosen).await?;

    if chosen != [5, 0] {
        return Err(Error::other(format!("the proxy {at} wants authentication, or is no SOCKS5 proxy")));
    }

    let mut request = vec![5, 1, 0, 3, name];
    request.extend(host.as_bytes());
    request.extend(port.to_be_bytes());
    stream.write_all(&request).await?;
    let mut reply = [0; 4];
    stream.read_exact(&mut reply).await?;

    if reply[1] != 0 {
        return Err(Error::new(ErrorKind::ConnectionRefused, format!("the proxy {at} could not reach {host}:{port} (SOCKS5 reply {})", reply[1])));
    }

    // The address it bound, which says nothing worth keeping.
    let skip = match reply[3] {
        1 => 4,
        4 => 16,
        3 => usize::from(stream.read_u8().await?),
        _ => return Err(Error::other(format!("the proxy {at} answered in no SOCKS5 it speaks"))),
    };
    let mut bound = vec![0; skip + 2];
    stream.read_exact(&mut bound).await?;

    Ok(stream)
}

/// Dials through Tor: onions, or with everything, every TCP address. It listens on nothing; the
/// onion's connections come to the node's TCP listener.
struct TorDial {
    proxy: String,
    everything: bool,
    timeout: Duration,
}

impl Transport for TorDial {
    type Output = tcp::tokio::TcpStream;
    type Error = std::io::Error;
    type ListenerUpgrade = std::future::Ready<Result<Self::Output, Self::Error>>;
    type Dial = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn listen_on(&mut self, _id: ListenerId, address: Multiaddr) -> Result<(), TransportError<Self::Error>> {
        Err(TransportError::MultiaddrNotSupported(address))
    }

    fn remove_listener(&mut self, _id: ListenerId) -> bool {
        false
    }

    fn dial(&mut self, address: Multiaddr, _opts: DialOpts) -> Result<Self::Dial, TransportError<Self::Error>> {
        let Some((host, port)) = target(&address, self.everything) else {
            return Err(TransportError::MultiaddrNotSupported(address));
        };
        let (proxy, timeout) = (self.proxy.clone(), self.timeout);

        Ok(Box::pin(async move {
            match tokio::time::timeout(timeout, socks5(&proxy, &host, port)).await {
                Ok(stream) => stream.map(tcp::tokio::TcpStream),
                Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("Tor did not reach {host}:{port}"))),
            }
        }))
    }

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Poll::Pending
    }
}

/// A transport that only listens: under --tor-only, the TCP listener, which must never dial, or
/// the node would show its IP.
struct ListenOnly<T>(T);

impl<T: Transport + Unpin> Transport for ListenOnly<T> {
    type Output = T::Output;
    type Error = T::Error;
    type ListenerUpgrade = T::ListenerUpgrade;
    type Dial = T::Dial;

    fn listen_on(&mut self, id: ListenerId, address: Multiaddr) -> Result<(), TransportError<Self::Error>> {
        self.0.listen_on(id, address)
    }

    fn remove_listener(&mut self, id: ListenerId) -> bool {
        self.0.remove_listener(id)
    }

    fn dial(&mut self, address: Multiaddr, _opts: DialOpts) -> Result<Self::Dial, TransportError<Self::Error>> {
        Err(TransportError::MultiaddrNotSupported(address))
    }

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Pin::new(&mut self.0).poll(cx)
    }
}

#[document(
    summary = "The node's transport, as its Tor settings say: QUIC and TCP directly, and onions through Tor with a proxy; or with only, TCP to listen on and everything dialed through Tor, no QUIC. TCP is secured with Noise and multiplexed with Yamux, as libp2p does it.",
    category = "Node",
    see_also = "TorConfig",
)]
pub fn transport(key: &libp2p::identity::Keypair, tor: &TorConfig) -> Result<Boxed<(PeerId, StreamMuxerBox)>, noise::Error> {
    let secured = |raw: Boxed<tcp::tokio::TcpStream>| -> Result<Boxed<(PeerId, StreamMuxerBox)>, noise::Error> {
        Ok(raw
            .upgrade(Version::V1)
            .authenticate(noise::Config::new(key)?)
            .multiplex(yamux::Config::default())
            .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
            .boxed())
    };
    let direct = || tcp::tokio::Transport::new(tcp::Config::default());
    let quic = || libp2p::quic::tokio::Transport::new(libp2p::quic::Config::new(key)).map(|(peer, connection), _| (peer, StreamMuxerBox::new(connection)));

    Ok(match (&tor.proxy, tor.only) {
        (None, _) => quic().or_transport(secured(direct().boxed())?).map(|either, _| either.into_inner()).boxed(),
        (Some(proxy), false) => {
            let tcp = direct().or_transport(TorDial { proxy: proxy.clone(), everything: false, timeout: tor.timeout }).map(|either, _| either.into_inner()).boxed();

            quic().or_transport(secured(tcp)?).map(|either, _| either.into_inner()).boxed()
        }
        (Some(proxy), true) => secured(ListenOnly(direct()).or_transport(TorDial { proxy: proxy.clone(), everything: true, timeout: tor.timeout }).map(|either, _| either.into_inner()).boxed())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONION: &str = "/onion3/vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd:4001";

    #[test]
    fn onions_are_dialed_through_tor_and_with_only_everything_tcp_is() {
        let onion: Multiaddr = ONION.parse().unwrap();
        let tcp: Multiaddr = "/ip4/203.0.113.7/tcp/4001".parse().unwrap();
        let named: Multiaddr = "/dns4/node.example/tcp/4001".parse().unwrap();
        let quic: Multiaddr = "/ip4/203.0.113.7/udp/4001/quic-v1".parse().unwrap();

        assert_eq!(target(&onion, false), Some(("vww6ybal4bd7szmgncyruucpgfkqahzddi37ktceo3ah7ngmcopnpyyd.onion".to_string(), 4001)));
        assert_eq!(target(&tcp, false), None, "directly, unless only");
        assert_eq!(target(&tcp, true), Some(("203.0.113.7".to_string(), 4001)));
        assert_eq!(target(&named, true), Some(("node.example".to_string(), 4001)), "the name, for Tor to resolve");
        assert_eq!(target(&quic, true), None, "Tor carries no UDP");
    }

    #[test]
    fn the_onion_is_read_once_tor_wrote_it() {
        let file = std::env::temp_dir().join(format!("chainv4-onion-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);

        assert_eq!(read_onion(&file), None, "not there yet");
        std::fs::write(&file, "not an address\n").unwrap();
        assert_eq!(read_onion(&file), None);
        std::fs::write(&file, format!("{ONION}\n")).unwrap();
        assert_eq!(read_onion(&file), Some(ONION.parse().unwrap()));

        // What GET /mining says of it: nothing until tor made it, then its host and the port.
        #[cfg(feature = "http")]
        let at = crate::api::MiningOnion::InFile { file: file.clone(), port: 4100 };
        let host = onion_host(&ONION.parse().unwrap()).unwrap();
        assert!(host.ends_with(".onion") && !host.contains(':') && !host.contains('/'), "{host}");
        #[cfg(feature = "http")]
        assert_eq!(at.address(), Some(format!("{host}:4100")));
        assert_eq!(onion_host(&"/ip4/127.0.0.1/tcp/4001".parse().unwrap()), None);
        #[cfg(feature = "http")]
        assert_eq!(crate::api::MiningOnion::InFile { file: file.with_extension("missing"), port: 4100 }.address(), None, "not there yet");
        #[cfg(feature = "http")]
        assert_eq!(crate::api::MiningOnion::At("x.onion:1".to_string()).address().as_deref(), Some("x.onion:1"));
        std::fs::remove_file(&file).unwrap();
    }

    #[tokio::test]
    async fn a_socks5_proxy_is_asked_for_the_name() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("socks5h://{}", listener.local_addr().unwrap());
        let served = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0; 3];
            stream.read_exact(&mut greeting).await.unwrap();
            stream.write_all(&[5, 0]).await.unwrap();
            let mut head = [0; 5];
            stream.read_exact(&mut head).await.unwrap();
            let mut asked = vec![0; usize::from(head[4]) + 2];
            stream.read_exact(&mut asked).await.unwrap();
            stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();

            (greeting, head, asked)
        });

        socks5(&proxy, "peer.onion", 4001).await.unwrap();
        let (greeting, head, asked) = served.await.unwrap();

        assert_eq!(greeting, [5, 1, 0]);
        assert_eq!(head[..4], [5, 1, 0, 3], "CONNECT to a name");
        assert_eq!(asked, [b"peer.onion".as_slice(), &4001u16.to_be_bytes()].concat());
    }

    #[tokio::test]
    async fn a_dial_through_a_proxy_that_never_answers_gives_up_at_the_tor_timeout() {
        // A proxy that takes the connection and says nothing, as a Tor that cannot build a circuit.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("socks5h://{}", listener.local_addr().unwrap());
        let _held = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(stream);
        });
        let mut dial = TorDial { proxy, everything: false, timeout: Duration::from_millis(300) };
        let started = std::time::Instant::now();
        let failed = dial.dial(ONION.parse().unwrap(), DialOpts { role: libp2p::core::Endpoint::Dialer, port_use: libp2p::core::transport::PortUse::New }).unwrap().await;

        assert_eq!(failed.err().map(|err| err.kind()), Some(std::io::ErrorKind::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(5), "at its own timeout, not the default 90 s");
        assert_eq!(TorConfig::default().timeout, TOR_TIMEOUT);
    }
}
