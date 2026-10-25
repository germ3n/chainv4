# Docker

`docker/` runs a chainv4 node behind nginx, with HTTPS from a certificate you give it, one from
Let's Encrypt, or one it makes itself. Three services, in `docker/compose.yaml`:

| Service | What it does |
| --- | --- |
| `node` | A node built with every feature: both consensus kinds, every kind of finality, both engines, the HTTP API with `--stream`, and both indexes. It serves the [explorer](explorer.md) at `/` and the [wallet](wallet.md) at `/wallet/`. |
| `nginx` | HTTPS in front of all of it, and port 80 for Let's Encrypt's challenges and a redirect to HTTPS. |
| `certbot` | Gets and renews the Let's Encrypt certificate. It only runs with `--profile letsencrypt`. |

## Quick start

```sh
cd docker
cp .env.example .env              # then set what you need
docker compose up -d --build
```

Then open `https://localhost/` for the explorer and `https://localhost/wallet/` for the wallet. The
certificate is self-signed at first, so the browser asks once.

The first build compiles the node, the explorer and the wallet in Linux, which takes a while.
Later builds reuse cargo's caches.

## The chain

**Made on the first start.** The node writes its genesis into the `chain` volume, and keeps it, so
the chain stays the same across restarts:

| Setting | Values |
| --- | --- |
| `CHAIN_CONSENSUS` | `authority` (the default): one key produces every block. `pow`: anyone mines. |
| `CHAIN_FINALITY` | `ffg` (the default): staked validators vote, proof of stake. `authority`: signers vote. `depth`: final once deep enough. `none`. |
| `CHAIN_FEES` | `base-fee` (the default), `bid`, or `free`. |

**Your own genesis.** A `docker/config/genesis.json` replaces those, as `chainv4 genesis` writes
one and you edit it (see [Genesis](genesis.md)).

**Changing the chain later** means a new chain: remove the `chain` volume, `docker volume rm
chainv4_chain`. `docker compose down -v` removes the certificates' volumes too.

**What the node does** follows from the genesis:
- **`BLOCKS=auto`:** it mines under proof of work, and produces under an authority. Or `mine`,
  `produce`, `off`.
- **`VALIDATE=auto`:** it votes under FFG and authority finality. Or `on`, `off`.

**Its keys:** `docker/config/producer.key` and `validator.key`, as `chainv4 keygen` writes them, or
`PRODUCER_KEY` and `VALIDATOR_KEY` naming other files. Without them it signs with the development
key, whose seed is public, and says so in its log. The node runs as uid 10001: on Linux, make the
key files readable to it.

**More:** `PEERS` (multiaddrs to dial), `MINE_THREADS`, `REWARD_TO`, `INDEX_EVENTS` and
`INDEX_ADDRESSES` (`genesis`, a height, `latest-N` or `off`), and `EXTRA_ARGS` for anything else
`chainv4` takes. `docker/entrypoint.sh` has them all.

## HTTPS

`TLS_MODE` picks where the certificate comes from, for `SERVER_NAME`:

| `TLS_MODE` | The certificate |
| --- | --- |
| `self-signed` (the default) | Made on the first start, for `SERVER_NAME` as a DNS name or an IP address, and kept in the `tls` volume. Made again when it is a day from expiring. |
| `provided` | Yours: `fullchain.pem` and `privkey.pem` in `docker/certs`. |
| `letsencrypt` | Let's Encrypt's, for `SERVER_NAME`. Start with `docker compose --profile letsencrypt up -d`, with `LETSENCRYPT_EMAIL` set. |

**How Let's Encrypt works here:**
- nginx starts with a self-signed stand-in, and answers HTTP-01 challenges on port 80 from a
  webroot it shares with certbot.
- certbot asks for the certificate, retrying every 5 minutes until it has one, then tries to renew
  it twice a day.
- nginx checks every minute for a new one and reloads when there is.

`LETSENCRYPT_STAGING=1` tries the staging service first, whose limits are looser. It needs a public
DNS name pointing at the host, and port 80 reachable from the internet.

**What nginx does:**
- It proxies everything to the node, and passes `/stream`, the node's server-sent events, on as
  they come.
- It takes bodies up to 4 MiB; the API's largest is 2 MiB.
- It sends `Strict-Transport-Security` only with a certificate browsers trust, never a
  self-signed one.
- Plain HTTP goes to HTTPS, keeping a port other than 443.

## What is exposed

| Port | What |
| --- | --- |
| `HTTPS_PORT` (443) | The API, the explorer, the wallet and `/stream`, over TLS. |
| `HTTP_PORT` (80) | Let's Encrypt's challenges, and a redirect to HTTPS. |
| `P2P_PORT` (4001, UDP) | Peer-to-peer, straight to the node. |

**The API is never published itself.** Port 8080 is only on compose's own network, for nginx.

**Peer-to-peer needs no proxy for its encryption.** It is libp2p over QUIC, which is always TLS
1.3, with certificates bound to each peer's id: every connection is encrypted and authenticated
end to end. nginx could not carry it anyway, as it is UDP.

## Keeping it

| Volume | What it holds |
| --- | --- |
| `chain` | The database and the genesis. |
| `tls` | The certificate nginx serves. |
| `letsencrypt` | certbot's account and certificates. |
| `acme` | The webroot for challenges. |

## Building

```sh
docker build -f docker/Dockerfile -t chainv4 .      # the node alone, from the repository's root
```

The image is built in three stages:
1. **The node** builds with `cargo build --release --locked` on `rust:1.98.1-bookworm`, with
   libclang for RocksDB's bindings.
2. **The web apps** build with trunk 0.22.0, whose download is checked against its checksum.
3. **The runtime** on `debian:bookworm-slim` holds the binary, both `dist` folders, `curl` for the
   health check, and `jq` for reading the genesis.

Cargo's caches are BuildKit cache mounts, so rebuilds are quick. It builds for the platform it runs
on, arm64 or amd64.

## What to keep in mind

- **The development key is public.** A chain made from the presets gives it the coin and every
  role. Fine to try things on; for a chain of value, bring a genesis and keys of your own.
- **A self-signed certificate is trusted only where you accept it.** Browsers ask once. The
  desktop explorer and wallet will not take it: use `provided` or `letsencrypt` for them.
- **The CLI speaks plain HTTP only,** so `chainv4 send` and `chainv4 light` cannot reach the node
  through nginx. Run them where they reach the API directly, inside the container:
  `docker compose exec node chainv4 send --node http://127.0.0.1:8080 …`.
- **One node per compose project.** Several nodes on one host want their own project names and
  ports: `docker compose -p second …` with other `HTTP_PORT`, `HTTPS_PORT` and `P2P_PORT`.
