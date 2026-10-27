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

**Workers:** with `docker/config/mining.token` (or `MINING_TOKEN` naming another file), a mining
node takes workers on TCP 4002, `MINING_PORT` on the host. A token someone else reads lets them
mine for the node, which pays the node, or report made-up hashrates, and nothing more: every nonce
is checked.

**Who reads the secrets:** `docker/pow.sh up` gives the token and `validator.key` to the node's
user in its container, uid 10001, with `podman unshare` under rootless Podman and as root
otherwise, readable by nobody else; where it can do neither, it leaves them readable to every user
and says so. The entrypoint stops with a message when a key is still unreadable, rather than leave
the node failing.

**Bootnodes:** `BOOTNODES`, separated by spaces, are this node's own, beside the chain's.
`PROTOCOL` names the chain's protocols (see [Node](node.md#networking)).

**More:** `PEERS` (multiaddrs to dial), `MINE_THREADS`, `REWARD_TO`, `INDEX_EVENTS` and
`INDEX_ADDRESSES` (`genesis`, a height, `latest-N` or `off`), and `EXTRA_ARGS` for anything else
`chainv4` takes. `docker/entrypoint.sh` has them all.

## HTTPS

`TLS_MODE` picks where the certificate comes from, for `SERVER_NAME`:

| `TLS_MODE` | The certificate |
| --- | --- |
| `self-signed` (the default) | Made on the first start, for `SERVER_NAME` as a DNS name or an IP address, and kept in the `tls` volume. Made again when it is a day from expiring. |
| `provided` | Yours: `fullchain.pem` and `privkey.pem` in `docker/certs`. |
| `letsencrypt` | Let's Encrypt's, for `SERVER_NAME`. Start with `docker/compose.sh --profile letsencrypt up -d`, which `docker/pow.sh up` does when `.env` says `letsencrypt`. `LETSENCRYPT_EMAIL` is optional: Let's Encrypt sends no expiry notices since 2025. With no domain of your own, `203-0-113-7.sslip.io` names the address `203.0.113.7`. |

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
| `MINING_PORT` (4002, TCP) | Workers, when `docker/config/mining.token` is there; nothing listens otherwise. |

**The API is never published itself.** Port 8080 is only on compose's own network, for nginx.

**Workers authenticate with the token.** Every message on the mining port carries an HMAC under
it, so no one without it can mine for the node or feed it nonces; the messages are not
encrypted, and need not be (see [Consensus](consensus.md#mining-with-workers)).

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
                                  #   [--finality ffg|authority|depth] [--governance council|electorate|none]
docker/pow.sh push user@your-vps -i ~/.ssh/vps  # here: the repository, genesis and .env
docker/pow.sh up                  # on the VPS, in that copy: build and start the stack
docker/pow.sh join your-vps --gpu # here: a node of the same chain that mines and dials the VPS
docker/pow.sh worker your-vps --gpu  # any rig: mine for the VPS with no chain of its own
docker/pow.sh status [url]        # either: the chain, height, peers and mining
docker/pow.sh logs                # on the VPS: follow the node's log
```

1. **`init`** makes a key from new words, printed once: restore them in the [wallet](wallet.md)
   to hold the account there. The key stays in `~/.chainv4-pow` (`POW_HOME`), never in the
   repository. It writes a genesis, the development PoW chain, whose coins and mining rewards are
   that key's, starting now, with no minter. It copies the genesis to
   `docker/config/genesis.json` and writes `docker/.env`, both ignored by git: mining rewards to
   the key, `--no-mdns`, and a self-signed certificate. It makes the mining token too, as `token`
   does. **Finality** (`--finality`, `FINALITY`):
   - **`ffg`** (the default): blocks go final by votes, in minutes. `init` makes a second key, a
     voting key, bonded at genesis for your account, and copies it to
     `docker/config/validator.key`; `push` takes that one key to the VPS, whose node votes with it
     around the clock. A stolen VPS can sign votes with it, never move your coins.
   - **`authority`**: the same voting key as the chain's one finality signer, for your account.
   - **`depth`**: blocks count as final once deep enough, and nothing votes, as before.

   Only the VPS votes: a second node voting with the same key could sign conflicting votes while
   the two are on different forks, which is slashable. `join` mines and does not vote.
   **Governance** (`--governance`, `GOVERNANCE`), under `ffg` or `authority`: `council` (the
   default) makes your account its one member, so you alone approve changes, such as new
   [bootnodes](governance.md#bootnodes), with `chainv4 send --key ~/.chainv4-pow/owner.key
   approve-change …`; `electorate` lets the validators approve by stake; `none` fixes the chain
   as genesis says.
2. **`push`** copies the repository by rsync, logging in with `-i KEY` and `-p PORT` when given
   (or `SSH_KEY` and `SSH_PORT`), without build output, `.git`, databases or keys, to
   `chainv4/` in the VPS's home (`REMOTE_DIR`). Files gone here are deleted there, so a source
   file moved is never built twice; what it leaves out, the chain, keys, certificates and build
   output, rsync never deletes.
3. **`up`** runs `docker/compose.sh up -d --build --force-recreate`, on Docker or Podman, so an
   update restarts on the new image (podman-compose keeps the old one otherwise), warns when the machine has under 4 GiB of memory
   and swap for the first build, waits for the node, and says which ports to open: **UDP 4001**
   for peers, TCP 80 and 443 for the explorer and wallet, in `ufw` and in the provider's firewall.
4. **`join`** mines on the CPU, or on the GPU with `--gpu` (`GPU=1`), and asks which in a
   terminal when neither is given; the GPU build has the `gpu` feature, in `target/gpu` of its
   own, so switching rebuilds neither. It builds the node here, keeps a copy of it in `POW_HOME`, looks the VPS's name up,
   and runs a node with the same genesis that dials `/ip4/…/udp/4001/quic-v1`, mines with
   `MINE_THREADS`, and serves its API, and the explorer and wallet when they are built, on
   `127.0.0.1:8090` (`API`). Ctrl-C stops it cleanly; `join` again carries on where it was.

5. **`token`** makes the token workers mine with, once: 32 random bytes in hex, in
   `POW_HOME/mining.token` and `docker/config/mining.token` (ignored by git), which `push` takes
   to the VPS. For a chain made before it: `token`, `push`, then `up` on the VPS, whose node then
   takes workers on **TCP 4002** (`MINING_PORT`); open that port too.
6. **`worker`** mines for the VPS's node on any machine with the token, keeping no chain: every
   GPU with `--gpu`, some with `--gpus 0,2`, or `MINE_THREADS` CPU threads with `--cpu`, asked in
   a terminal when none is given. It reads the token from `POW_HOME`, or `TOKEN`: copy it to each
   rig. What the workers find pays the VPS's `REWARD_TO`, the same key.

Both mine, so both earn rewards, to the same key. A chain with its own `PROTOCOL` in `.env`
needs the same `PROTOCOL` set for `join` and `worker`. `SYMBOL` names the coin at `init`, `NAMES` the
suffix of its [names](names.md), the symbol in lower case unless set or `none` for none, `P2P_PORT`
moves the QUIC port, and `docker/pow.sh` alone lists every setting.

### A new chain on the same keys

A genesis is fixed with its chain: what a newer build's genesis adds, such as [names](names.md) or
the EVM, comes to a running chain only as a new chain. `regenesis` makes one for the keys `init`
made, so the seed words, the VPS's voting key and the council stay:

```sh
docker/pow.sh regenesis                 # here: shows what moves aside, and asks for yes
docker/pow.sh push root@203.0.113.7 -i ~/.ssh/vps
# on the VPS:
docker/pow.sh reset                     # stops the node, moves its chain aside in the volume
docker/pow.sh up
# here again:
docker/pow.sh join 203.0.113.7 --gpu
```

- **What moves aside, kept:** `docker/config/genesis.json`, and `genesis.json` and `chain.db` in
  `POW_HOME`, each to `.old-` and the time. `reset` moves the VPS's `/data/chain.db` the same
  way, inside the `chain` volume, and `/data/genesis.json`: the container keeps the genesis it
  started with there, so a new one in `docker/config` alone would change nothing. Nothing is deleted.
- **What carries over:** the owner, the voter bonded for it, the council and the coins at genesis,
  and the finality and governance kinds, unless `--finality` and `--governance` say others. The
  balances and names of the old chain do not: it is a new chain, from block 0.
- **Asked first.** Both say what they move and wait for `yes`; `--yes` answers for a script.
- **Everyone else** on the old chain needs the new genesis, and the wallet's pinned genesis for
  the node changes: it follows the new chain once you forget the old one in its Settings.

### A faucet

`docker/pow.sh faucet https://your-vps --fund 1000`, here, makes a key for a
[faucet](faucet.md), funds it from your key through the VPS's node, copies it to
`docker/config/faucet.key`, and adds `FAUCET_AMOUNT`, `FAUCET_EVERY` and `FAUCET_WORK` to
`docker/.env`. `push` takes the key along, as it does the voting key, and `up` hands it to the
node's user; the node then serves `POST /faucet`, and the wallet and explorer it serves show it.

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
| `chain` | The database, the genesis, and the node's network key, `net.key`, so its peer id survives a rebuild. |
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
- **The CLI reaches the node through nginx too.** `chainv4 send` and `chainv4 light` take
  `--node https://…`. A certificate from Let's Encrypt, or a CA you trust, needs nothing more;
  for the self-signed one, give it to them with `--ca-cert`, after fetching it once:
  `docker/compose.sh exec nginx cat /etc/chainv4/tls/fullchain.pem > vps.pem`, then
  `chainv4 send --node https://203.0.113.7 --ca-cert vps.pem …`. `--insecure` skips the check,
  for trying things out only. A self-signed certificate an older `tls.sh` made was a CA, which
  rustls refuses as a server's own; the nginx container makes it again, as one that is not, when
  it next starts.
- **A VPS takes the peers' word for nothing.** Its chain is the genesis it was given: a node
  dialing it with another genesis is on another chain, and neither syncs from the other.
- **One node per compose project.** Several nodes on one host want their own project names and
  ports: `docker/compose.sh -p second …` with other `HTTP_PORT`, `HTTPS_PORT` and `P2P_PORT`.
