# Docker

`docker/` runs a chainv4 node behind nginx, with HTTPS from a certificate you give it, one from
Let's Encrypt, or one it makes itself, on Docker or Podman (see [Podman](#podman)). Three
services, in `docker/compose.yaml`:

| Service | What it does |
| --- | --- |
| `node` | A node built with every feature: every kind of consensus and finality, both engines, the HTTP API with `--stream`, and both indexes. It serves the [explorer](explorer.md) at `/` and the [wallet](wallet.md) at `/wallet/`. |
| `nginx` | HTTPS in front of all of it, and port 80 for Let's Encrypt's challenges and a redirect to HTTPS. |
| `certbot` | Gets and renews the Let's Encrypt certificate. It only runs with `--profile letsencrypt`. |

## Quick start

```sh
cp docker/.env.example docker/.env    # then set what you need
docker/compose.sh up -d --build
```

`docker/compose.sh` runs Docker's compose, or Podman's when `CONTAINER_ENGINE=podman`; from
`docker/`, `docker compose` and `podman compose` take the file as it is too.

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
chainv4_chain`. `docker/compose.sh down -v` removes the certificates' volumes too.

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
| `letsencrypt` | Let's Encrypt's, for `SERVER_NAME`. Start with `docker/compose.sh --profile letsencrypt up -d`, with `LETSENCRYPT_EMAIL` set. |

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

## A proof-of-work chain across machines

`docker/pow.sh` runs one chain on a VPS and on your own machine: the VPS in Docker with its QUIC
port open to the internet, and your machine as a node that mines too and dials it. Your machine
sits behind NAT, so the VPS cannot dial it, but the one connection carries blocks both ways, and
the node dials the VPS again every 15 seconds while it is cut off.

```sh
docker/pow.sh init                # here, once: a key of your own, and a genesis that pays it
docker/pow.sh push user@your-vps  # here: the repository, the genesis and .env, by rsync
docker/pow.sh up                  # on the VPS, in that copy: build and start the stack
docker/pow.sh join your-vps       # here: a node of the same chain that mines and dials the VPS
docker/pow.sh status [url]        # either: the chain, height and peers
```

1. **`init`** makes a key from new words, printed once: restore them in the [wallet](wallet.md)
   to hold the account there. The key stays in `~/.chainv4-pow` (`POW_HOME`), never in the
   repository. It writes a genesis, the development PoW chain with depth finality, so nothing
   votes, whose coins, minting and mining rewards are that key's, starting now. It copies the
   genesis to `docker/config/genesis.json` and writes `docker/.env`, both ignored by git: mining
   rewards to the key, `--no-mdns`, and a self-signed certificate.
2. **`push`** copies the repository by rsync, without build output, `.git`, databases or keys, to
   `chainv4/` in the VPS's home (`REMOTE_DIR`).
3. **`up`** runs `docker/compose.sh up -d --build`, on Docker or Podman, warns when the machine has under 4 GiB of memory
   and swap for the first build, waits for the node, and says which ports to open: **UDP 4001**
   for peers, TCP 80 and 443 for the explorer and wallet, in `ufw` and in the provider's firewall.
4. **`join`** builds the node here, keeps a copy of it in `POW_HOME`, looks the VPS's name up,
   and runs a node with the same genesis that dials `/ip4/…/udp/4001/quic-v1`, mines with
   `MINE_THREADS`, and serves its API, and the explorer and wallet when they are built, on
   `127.0.0.1:8090` (`API`). Ctrl-C stops it; `join` again carries on where it was.

Both mine, so both earn rewards, to the same key. `SYMBOL` names the coin at `init`, `P2P_PORT`
moves the QUIC port, and `docker/pow.sh` alone lists every setting.

## Podman

`CONTAINER_ENGINE=podman`, in the environment or `docker/.env`, has `docker/compose.sh` and
`docker/pow.sh up` run Podman: `podman compose`, or `podman-compose` where that is missing.
Unset, they take Docker if it is installed, else Podman. The files are the same for both:
- **Images are named with their registry,** `docker.io/library/rust` and so on, as Podman refuses
  short names it cannot resolve without asking.
- **The health check is in the compose file too,** since an image Podman builds in OCI's format
  keeps no `HEALTHCHECK`, and nginx waits for the node to be healthy. `compose.sh` builds in
  Docker's format all the same (`BUILDAH_FORMAT=docker`).
- **Bind mounts carry `:z`,** which relabels them for SELinux on Fedora and RHEL hosts, Podman's
  usual ones, and changes nothing elsewhere.
- **The build's architecture** comes from the build machine when the builder sets no
  `TARGETARCH`, as older Buildah does not.

**Rootless Podman binds no port below 1024** by default, so not nginx's 80 and 443. Either lower
the bound, `sudo sysctl net.ipv4.ip_unprivileged_port_start=80` and the same in `/etc/sysctl.d`,
or set `HTTP_PORT=8080` and `HTTPS_PORT=8443`; `compose.sh` says so when it would fail. The
node's UDP port, 4001, is above it.

**Restarting after a reboot:** `restart: unless-stopped` brings containers back only once
Podman's restart service runs: `systemctl --user enable podman-restart.service`, with
`loginctl enable-linger` for the user, so it runs without a login.

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
  `docker/compose.sh exec node chainv4 send --node http://127.0.0.1:8080 …`.
- **A VPS takes the peers' word for nothing.** Its chain is the genesis it was given: a node
  dialing it with another genesis is on another chain, and neither syncs from the other.
- **One node per compose project.** Several nodes on one host want their own project names and
  ports: `docker/compose.sh -p second …` with other `HTTP_PORT`, `HTTPS_PORT` and `P2P_PORT`.
