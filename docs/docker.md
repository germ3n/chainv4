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

**A pool:** `POOL=1` in `docker/.env` makes the mining port a mining pool, which pays each worker
for its shares, through an escrow it claims from (see [Mining pools](consensus.md#mining-pools));
`POOL_OPEN=1` makes it an open one, which anyone joins with a payout address and no token.
`POOL_FEE` is the percent the pool keeps, `POOL_WINDOW` how many blocks' worth of shares each block
shares out by, and `POOL_PUSH` with `docker/config/pool-push.key` (or `POOL_PUSH_KEY`) has the node
claim for miners owed that much. A rig joins with `PAYOUT=ADDRESS docker/pow.sh worker HOST`, and
keeps its claims in `POW_HOME`'s `pool-claims.json`, which `chainv4 claim --saved` claims by from
any node should the pool's be gone.

**Mining alone:** `MINING_SOLO=1` has the mining port take workers mining alone too, each block one
finds paying it in full (see [Mining alone](consensus.md#mining-alone)); `MINING_OPEN=1` opens the
port to anyone with a payout address, beside `POOL=1` or `MINING_SOLO=1`, as `POOL_OPEN=1` does for
a pool. `MINING_PUBLIC=HOST:PORT` is where the apps are told to reach the port, `GET /mining`, when
it is not the API's host and port 4002; with `TOR=1` and `TOR_MINING=1`, an app on the onion is told
the onion's port on `MINING_PORT` instead, as soon as tor has made the onion. nginx passes `/mining/ws`, the port over a WebSocket, on
to the node, behind its TLS. Every miner on an open port comes through it, as its token is public and
anyone on the way could rewrite a payout over plain TCP ([why](consensus.md#mining-with-workers)):
the web wallet, the desktop and phone wallets, and `chainv4 worker --node https://YOUR-HOST`, or
`PAYOUT=ADDRESS docker/pow.sh worker https://YOUR-HOST`. The TCP port needs opening only for a private
port's workers, which stay on it, and the `--insecure-open-port` of whoever insists. With a
self-signed certificate workers need `--ca-cert vps.pem`, the certificate as below: Let's Encrypt's needs
nothing.

**Peers that dial too early:** a peer's QUIC packets that arrive before the node's port mapping is
up leave a UDP entry in the host's connection tracking that skips the mapping, and its retries,
always from the same port, keep that entry alive, so the dial times out for as long as it is
retried, while the node's own dial the other way works. `docker/pow.sh up` clears those entries with
`conntrack` once the node answers; after restarting a node any other way, run `conntrack -D -p udp
--orig-port-dst 4001`. A node also stops dialing a `--peer` that has connected to it first.

**What it shares:** `REPORT` is the node's [`--report`](node.md#what-a-node-shares-of-itself),
`public` unless set, and `HIDE` and `SHOW` its `--hide` and `--show`, by commas.
`docker/pow.sh report-token` makes a token (`docker/config/report.token`, or `REPORT_TOKEN` naming
another file) that shows its bearer everything: `docker/pow.sh status` sends it, and it goes in the
explorer's Settings. nginx and Tor are other containers, so what comes through them is never taken
for the node's own machine.

**Who reads the secrets:** `docker/pow.sh up` gives the token and `validator.key` to the node's
user in its container, uid 10001, with `podman unshare` under rootless Podman and as root
otherwise, readable by nobody else; where it can do neither, it leaves them readable to every user
and says so. The entrypoint stops with a message when a key is still unreadable, rather than leave
the node failing.

**Bootnodes:** `BOOTNODES`, separated by spaces, are this node's own, beside the chain's.
`PROTOCOL` names the chain's protocols (see [Node](node.md#networking)).

**More:** `PEERS` (multiaddrs to dial), `MINE_THREADS`, `REWARD_TO`, `INDEX_EVENTS` and
`INDEX_ADDRESSES` (`genesis`, a height, `latest-N` or `off`), `INDEX_MULTISIGS` (`on`, the default,
or `off`), and `EXTRA_ARGS` for anything else
`chainv4` takes. `docker/entrypoint.sh` has them all.

**The node's limits,** each the flag of its name and passed on only when set, so one left empty keeps
chainv4's own default (see [Node](node.md#limits-and-tuning) for defaults and bounds):
- the mempool's: `MEMPOOL_SIZE`, `MEMPOOL_PER_SENDER`, `MAX_NONCE_GAP`, `REPLACE_BUMP_PERCENT` and
  `UNDERPRICED_FOR`;
- sync's: `SYNC_BATCH_BYTES`;
- peers': `MAX_PEERS_IN`, `BAN_SCORE`, `BAN_WINDOW`, `MAX_STRIKES`, `STRIKE_WINDOW` and `TOR_TIMEOUT`;
- the API's: `HTTP_RATE`, `API_MAX_PAGE` and `SUGGEST_MULTIPLIER`. nginx in front already limits each
  address to `RATE_API` a second; `HTTP_RATE` is the node's own, per client as nginx names it, for a
  node reached around nginx too;
- the engines': `LUA_STATES`, `LUA_BUILDERS`, `WASM_CACHE_MB` and `WASM_CACHE_MODULES`;
- `UPDATE_EVERY`, and `TUNE`, `KEY=VALUE,...` for `--tune`.

`docker/.env.example` lists each with its default.

### Firewall

With `ufw` on, rootful Podman's containers stop working until it is told about them: they get no DNS,
cannot reach out, and no published port answers, as they all go through forwarding, which `ufw` denies
by default. `ufw allow PORT` does not help: it rules what comes to the machine itself. As root, on
Podman's default subnets:

```sh
ufw default deny incoming && ufw default allow outgoing
ufw allow 22/tcp
ufw allow from 10.88.0.0/13 to any port 53                 # containers' DNS, on this machine
ufw route allow from 10.88.0.0/13                          # their way out: builds need it
ufw route allow proto udp to 10.88.0.0/13 port 4001        # peers
ufw route allow proto tcp to 10.88.0.0/13 port 80          # a public node only, as the next two
ufw route allow proto tcp to 10.88.0.0/13 port 443
ufw route allow proto tcp to 10.88.0.0/13 port 4002        # mining
ufw --force enable
```

- **The port is the one inside the container:** a route rule is checked after the port is mapped.
- **A node that is not public** keeps 80, 443 and 4002 shut this way, and is reached over `ssh`.
- **Check from outside,** with `nc -z HOST PORT`: `ufw status` says what was asked for, not what answers.

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
| An onion, with `TOR=1` | The API, the explorer and the wallet over Tor, and with `TOR_MINING=1` the mining port (see [An onion service](#an-onion-service)). Nothing is published for it: Tor reaches out. |

**nginx limits what each address may ask.** Past a limit it answers 429 with a JSON `error`, which the
apps show and retry after, and a `Retry-After` of a second:

| What | A second | Burst | Who shares it |
| --- | --- | --- | --- |
| The API, the explorer and the wallet (`RATE_API`) | 30 | 180 | Each address |
| Contract calls, `POST /call` and `/rpc` (`RATE_CALLS`), on top | 5 | 20 | Each address |
| Blocks with proofs, `GET /blocks/range` (`RATE_RANGE`), on top | 5 | 20 | Each address |
| Everything over the onion | 90 | 360 | Everyone on it, as all come from tor |
| Connections | 64 at once, 8 for `/stream`, 32 for `/mining/ws`, as many as the mining port lets a rig run | | Each address |

- **Set in `.env`:** `RATE_API`, `RATE_CALLS` and `RATE_RANGE`, the bursts and the onion's following them, and
  `RATE_LIMIT=off` for none. A restart of nginx takes them up.
- **What is not limited here:** peers and the mining port, which do not pass through nginx. Over the
  onion, contract calls and block ranges have only the onion's limit, so someone there can still hold up the API's thread.
  The node's own bound on [block ranges](api.md#get-blocksrangefromhcountnentrye) counts everyone on the onion
  as one client, so two are under way there at a time, and the rest are told to come back.
- **Behind a shared address,** as a campus or a mobile carrier's, everyone shares a limit: raise `RATE_API` if they meet it.

**The API is never published itself.** Port 8080 is only on compose's own network, for nginx.

**Workers authenticate with the token.** Every message on the mining port carries an HMAC under
it, so no one without it can mine for the node or feed it nonces; the messages are not
encrypted, and on a private port need not be. An open port's token is public, so its workers go
over `/mining/ws` behind TLS (see [Consensus](consensus.md#mining-with-workers)).

**Peer-to-peer needs no proxy for its encryption.** It is libp2p over QUIC, which is always TLS
1.3, with certificates bound to each peer's id: every connection is encrypted and authenticated
end to end. nginx could not carry it anyway, as it is UDP.

## An onion service

With `TOR=1` in `.env`, `docker/pow.sh up` runs Tor beside the node (`docker/tor`, the compose
profile `tor`), and the node is also an onion service: `http://ADDRESS.onion` serves its API, the
explorer and the wallet, as `HTTPS_PORT` does; `P2P_PORT` there takes peers over TCP, which the node
then listens for beside QUIC and tells peers as `/onion3/ADDRESS:P2P_PORT`; and with
`TOR_MINING=1` its mining port as well, on `MINING_PORT`. The node dials onion peers through tor's
SOCKS port, which only compose's network may use.

```sh
# in docker/.env
TOR=1
TOR_MINING=1
```

```sh
docker/pow.sh up       # on the VPS: tor starts with the rest, and prints the onion when it made it
docker/pow.sh onion    # on the VPS: the onion's address again
```

- **No TLS, and none needed:** an onion address is the hash of the service's own key, and Tor
  encrypts and authenticates every connection to it end to end, so `http://` is as private as
  HTTPS and needs no certificate. Tor goes straight to the node's port 8080 on compose's network,
  not through nginx.
- **The address stays** while the `tor` volume does: it holds the onion's key. Removing the volume
  makes a new address; backing it up keeps the old one on another machine.
- **Tor's own SOCKS port is off** in the container: it serves the onion and is no proxy for the
  machine.
- **Peer-to-peer goes both ways:** over QUIC on `P2P_PORT` as before, for nodes on the internet,
  and over TCP at the onion, for nodes behind Tor. `docker/pow.sh join ADDRESS.onion` joins it that
  way, through the Tor on your machine alone, never showing your IP (see
  [Over Tor](node.md#over-tor)).
- **Reaching it:** Tor Browser opens the explorer and the wallet as any site; the apps reach it
  through a SOCKS proxy, Settings › Proxy (**Orbot**, socks5h://127.0.0.1:9050, or **Tor
  Browser**, 9150), which resolves the `.onion` name inside Tor; the CLI takes `--proxy`; workers
  mine through it with `chainv4 worker --node ADDRESS.onion:4002 --proxy socks5h://127.0.0.1:9050`,
  or `docker/pow.sh worker ADDRESS.onion`, which uses Tor's port by itself.

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
   vote-change --vote yes …`; `electorate` lets the validators approve by stake; `none` fixes the chain
   as genesis says.
2. **`push`** copies the repository by rsync, logging in with `-i KEY` and `-p PORT` when given
   (or `SSH_KEY` and `SSH_PORT`), without build output, `.git`, databases or keys, to
   `chainv4/` in the VPS's home (`REMOTE_DIR`). Files gone here are deleted there, so a source
   file moved is never built twice; what it leaves out, the chain, keys, certificates and build
   output, rsync never deletes.
3. **`up`** runs `docker/compose.sh up -d --build --force-recreate`, on Docker or Podman, so an
   update restarts on the new image (podman-compose keeps the old one otherwise), warns when the machine has under 4 GiB of memory
   and swap for the first build, waits for the node, and says which ports to open: **UDP 4001**
   for peers, TCP 80 and 443 for the explorer and wallet, in the provider's firewall and in
   [`ufw`](#firewall), whose ordinary `allow` opens nothing for them.
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

**Rootless Podman hides who is connecting.** With compose's bridge network, every connection reaches
a container from the network's gateway, `10.89.x.x`, so faucet limits, IP bans, the pool's view of
its workers and [what a node shares](node.md#what-a-node-shares-of-itself) go by that one address. So
a public node runs rootful, with the [firewall](#firewall) above; rootless suits a node that takes
no public traffic.

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

### Building once for several servers

`docker/pow.sh up` builds on the machine it runs on, so with three servers a deploy compiles the
same images three times. To build once instead, run `up` on one server, check it, then copy its
images to the others and start them there without a build:

```sh
docker/pow.sh images root@203.0.113.7 root@203.0.113.8 root@203.0.113.9 -i ~/.ssh/vps   # here
# on 203.0.113.8 and 203.0.113.9:
docker/pow.sh up --no-build
```

- **What is copied:** the node's image and nginx's, which hold the build. Tor's holds none, so
  `up --no-build` makes it on the server where `TOR=1`. The two are
  saved on the source, gzipped, streamed through this machine by SSH and loaded on each target:
  about 170 MB and 70 MB. Nothing is stored on the way, no server holds a key to another, and
  there's no registry or token.
- **Checked:** each target must end up with the same image ids as the source, or `images` stops.
- **`up --no-build`** starts what is there, compiling nothing, and refuses if the node's or
  nginx's image is missing. The servers must share an architecture: an image built on x86_64 runs
  only on x86_64.
- **Building where it runs** stays the default, for a server on its own.

### Pulling what CI built

The **image** workflow (Actions, run by hand) builds the node's image and nginx's once, on
GitHub's runners, and pushes them to the repository's container registry, private as the
repository is: `ghcr.io/welikethestock/chainv4` and `…/chainv4-nginx`, tagged `sha-` and the
commit's first 12 hex digits, `latest`, and a version when the run is given one. A server then
pulls them and starts them, compiling nothing:

```sh
docker/pow.sh login welikethestock   # once per server: paste a token, then Ctrl-D
docker/pow.sh up --pull sha-1a2b3c4d5e6f
```

- **The token** is a GitHub token with `read:packages` alone, so a server that is taken can pull
  images and nothing else. It is kept by Docker or Podman's own login, not by `pow.sh`.
- **What is pulled** is tagged `chainv4:latest` and `chainv4-nginx:latest` here, as a build would,
  so `up --no-build` and `images` go on working with it.
- **Another registry:** `IMAGE_REPO` in `docker/.env` or the environment.
- **x86_64 only,** as the servers are; an arm64 server builds its own.

## What to keep in mind

- **A preset chain's name** comes from `CHAIN_NAME` in `docker/.env`, written into its genesis on
  the first start; `docker/pow.sh init` and `regenesis` take `CHAIN_NAME` too, `chainv4 testnet`
  unless set. The apps show it wherever they name the network.
- **The image never updates itself.** A node in it warns while a newer [release](updates.md) is
  out, and stops at an upgrade it is too old for; update by building or pulling a new image. Set
  `UPDATES` in `docker/.env` to look for releases at another manifest, or `off` for none.
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
