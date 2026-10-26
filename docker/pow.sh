#!/bin/sh
# A proof-of-work chain across two machines: a VPS that runs the Docker stack with its QUIC port
# open to the internet, and this machine, which mines too and dials it. The VPS cannot dial back
# through this machine's NAT, but the one connection carries blocks both ways, and a node dials a
# --peer again every 15 seconds while it is cut off. Run from anywhere:
#
#   docker/pow.sh init                here, once: a key of your own, and a genesis that pays it,
#      [--finality ffg|authority|depth]  final by the VPS's votes (ffg, the default), or by depth;
#      [--governance council|electorate|none]  and who may change it later: you (council)
#   docker/pow.sh push user@host      here: the repository, the genesis and .env, to the VPS;
#                  [-i KEY] [-p PORT]  -i the SSH private key to log in with, -p its port
#   docker/pow.sh up                  on the VPS, in that copy: build and start the stack, with
#                                     Docker or Podman
#   docker/pow.sh join host           here: a node of the same chain that mines and dials the VPS,
#                  [--gpu | --cpu]     on the GPU or the CPU; asked when neither is given
#   docker/pow.sh token               here, once: the token workers mine for the VPS with; push
#                                     takes it there, and up starts taking workers
#   docker/pow.sh worker host         any rig: mine for the VPS's node over its mining port, with
#      [--gpu [--gpus 0,2] | --cpu]    no chain of its own: every GPU, those picked, or threads
#   docker/pow.sh status [url]        either: the node's chain, height, peers and mining
#   docker/pow.sh logs                on the VPS: follow the node's log
#
# The key stays here, in POW_HOME (~/.chainv4-pow): of what init makes, only the genesis goes to
# the VPS. Settings, all optional:
#
#   POW_HOME       where the key, genesis, this machine's chain and binary live
#   SYMBOL         the coin's ticker, for init: DEV unless set
#   FINALITY       init's --finality; GOVERNANCE its --governance
#   SERVER_NAME    the VPS's domain or address, for init's .env and its certificate
#   MINE_THREADS   how many threads mine, here and on the VPS: 1 unless set
#   GPU            1 to mine on the GPU at join, 0 on the CPU, as --gpu and --cpu
#   P2P_PORT       the VPS's QUIC port: 4001 unless set
#   API            this machine's API, for join and status: 127.0.0.1:8090 unless set
#   REMOTE_DIR     where push puts the repository on the VPS: chainv4 under its home
#   SSH_KEY        push's SSH private key, as -i gives it; SSH_PORT its port, as -p
#   MINING_PORT    the VPS's mining port, for worker: 4002 unless set
#   TOKEN          worker's token file: POW_HOME's mining.token, else docker/config's
#   PROTOCOL       the chain's protocol name, for join and worker, as the VPS's .env has it
#   EXTRA_ARGS     anything else for join's node, such as --no-mdns
#   CONTAINER_ENGINE  docker or podman, for up: as docker/compose.sh picks it, unset
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
home=${POW_HOME:-$HOME/.chainv4-pow}
p2p_port=${P2P_PORT:-4001}
mining_port=${MINING_PORT:-4002}

say() { printf '[pow] %s\n' "$*" >&2; }
fail() { printf '[pow] %s\n' "$*" >&2; exit 1; }

# The release node, built here and kept in POW_HOME: an editor's cargo check may clear target/,
# and a running node keeps its binary anyway. Copied in by renaming, as macOS kills a binary run
# again after it was overwritten where it lay.
# With gpu, the build with the gpu feature, in a target folder of its own, so switching between
# the two rebuilds neither.
binary() {
    case "${1:-}" in
        gpu) name=chainv4-gpu; built="$root/target/gpu/release/chainv4"; flags="--features gpu --target-dir $root/target/gpu" ;;
        *) name=chainv4; built="$root/target/release/chainv4"; flags= ;;
    esac

    if [ ! -x "$home/bin/$name" ] || [ "$built" -nt "$home/bin/$name" ] || [ "${REBUILD:-}" = 1 ]; then
        say "building the node${flags:+ with GPU mining}"
        # shellcheck disable=SC2086
        (cd "$root" && cargo build --release --locked --quiet $flags)
        mkdir -p "$home/bin"
        cp "$built" "$home/bin/$name.new"
        mv -f "$home/bin/$name.new" "$home/bin/$name"
    fi

    echo "$home/bin/$name"
}

owner() {
    [ -s "$home/owner.address" ] || fail "no key yet: run docker/pow.sh init first"
    cat "$home/owner.address"
}

init() {
    finality=${FINALITY:-ffg}
    governance=${GOVERNANCE:-}

    while [ $# -gt 0 ]; do
        case "$1" in
            --finality) [ $# -ge 2 ] || fail "--finality takes ffg, authority or depth"; finality=$2; shift 2 ;;
            --governance) [ $# -ge 2 ] || fail "--governance takes council, electorate or none"; governance=$2; shift 2 ;;
            *) fail "init takes --finality and --governance, not $1" ;;
        esac
    done

    case "$finality" in
        ffg | authority) governance=${governance:-council} ;;
        depth)
            [ "${governance:-none}" = none ] || fail "governance needs finality that votes: --finality ffg or authority"
            governance=none
            ;;
        *) fail "--finality is ffg, authority or depth, not $finality" ;;
    esac

    case "$governance" in
        council | electorate | none) ;;
        *) fail "--governance is council, electorate or none, not $governance" ;;
    esac

    command -v python3 > /dev/null || fail "init needs python3, to write the genesis"
    [ -e "$home/owner.key" ] && fail "$home/owner.key is there already: this machine's chain is made. Remove $home to start another"
    [ -s "$root/docker/config/genesis.json" ] && fail "docker/config/genesis.json is there already, for another chain: move it away first"

    mkdir -p "$home"
    chmod 700 "$home"
    chainv4=$(binary)

    say "a key of your own, from new words: restore them in the wallet to hold this account there"
    "$chainv4" keygen --phrase --out "$home/owner.key" | tee "$home/keygen.out" | grep -v '^address ' >&2
    sed -n 's/^address //p' "$home/keygen.out" > "$home/owner.address"
    rm -f "$home/keygen.out"
    [ -s "$home/owner.key" ] && [ -s "$home/owner.address" ] || fail "the key was not made"
    chmod 600 "$home/owner.key"
    address=$(owner)

    # A voting key of its own, for the VPS to vote with around the clock: bonded at genesis for
    # your account, so a stolen VPS can sign votes, never move your coins.
    voter=
    if [ "$finality" != depth ]; then
        "$chainv4" keygen --out "$home/validator.key" > "$home/keygen.out"
        voter=$(sed -n 's/^address //p' "$home/keygen.out")
        rm -f "$home/keygen.out"
        [ -n "$voter" ] && [ -s "$home/validator.key" ] || fail "the voting key was not made"
        chmod 600 "$home/validator.key"
    fi

    # The development PoW chain with this finality and governance; then its coins, the minting,
    # the voters and the members, and the time it starts, are this key's and now.
    "$chainv4" genesis --consensus pow --finality "$finality" --governance "$governance" --fees base-fee --out "$home/genesis.json" > /dev/null
    python3 -I - "$home/genesis.json" "$address" "${SYMBOL:-DEV}" "$voter" <<'EOF'
import json, sys, time
path, address, symbol, voter = sys.argv[1:]
with open(path) as f:
    genesis = json.load(f)
genesis["allocations"] = [{"address": address, "amount": str(10**6 * 10**genesis["coin"]["decimals"])}]
# Mint is for development chains alone, signed by the development key.
genesis["minter"] = None
genesis["coin"]["symbol"] = symbol
genesis["timestamp"] = int(time.time() * 1000)
finality = genesis.get("finality") or {}
if finality.get("type") == "ffg":
    finality["validators"] = [{"account": address, "voter": voter, "bond": finality["validators"][0]["bond"]}]
elif finality.get("type") == "authority":
    finality["signers"] = [{"account": address, "key": voter}]
governance = genesis.get("governance") or {}
if governance.get("type") == "council":
    governance["members"] = [address]
with open(path, "w") as f:
    json.dump(genesis, f, indent=2)
    f.write("\n")
EOF
    cp "$home/genesis.json" "$root/docker/config/genesis.json"

    if [ -n "$voter" ]; then
        (umask 077 && cp "$home/validator.key" "$root/docker/config/validator.key")
    fi

    token

    if [ -e "$root/docker/.env" ]; then
        say "docker/.env is there already: set REWARD_TO=$address in it"
    else
        cat > "$root/docker/.env" <<EOF
# Written by docker/pow.sh init. The chain is docker/config/genesis.json: a proof-of-work chain
# with $finality finality and $governance governance. Under ffg or authority the node votes with
# docker/config/validator.key, as the entrypoint finds it.
# docker or podman, as docker/compose.sh runs it; unset, whichever the VPS has.
CONTAINER_ENGINE=${CONTAINER_ENGINE:-}
REWARD_TO=$address
MINE_THREADS=${MINE_THREADS:-1}
PEERS=
INDEX_EVENTS=genesis
INDEX_ADDRESSES=genesis
# A VPS's network holds none of your nodes: peers dial it.
EXTRA_ARGS=--no-mdns

# The name every protocol starts with; join and worker need the same, as PROTOCOL.
PROTOCOL=${PROTOCOL:-}

# HTTPS in front: self-signed until you give the VPS a domain and use letsencrypt.
TLS_MODE=self-signed
SERVER_NAME=${SERVER_NAME:-localhost}
HTTP_PORT=80
HTTPS_PORT=443
P2P_PORT=$p2p_port
MINING_PORT=$mining_port
EOF
    fi

    say "made $home: the key (owner.key, which never leaves this machine) and the genesis"
    say "coins and mining rewards go to $address"

    case "$finality" in
        ffg) say "blocks go final by the VPS's votes, with validator.key, bonded for your account; push takes it there" ;;
        authority) say "blocks go final by the VPS's votes, with validator.key, the one signer, for your account; push takes it there" ;;
        depth) say "blocks count as final once they are deep enough; nothing votes" ;;
    esac

    case "$governance" in
        council) say "you alone approve changes, such as new bootnodes: chainv4 send --key $home/owner.key approve-change …" ;;
        electorate) say "the validators approve changes by stake" ;;
    esac

    say "next: docker/pow.sh push user@your-vps"
}

# The token workers mine with: made once, kept in POW_HOME for workers here, and in docker/config
# for push to take to the VPS.
token() {
    if [ ! -s "$home/mining.token" ]; then
        mkdir -p "$home"
        chmod 700 "$home"
        # 32 random bytes, in hex: what openssl rand -hex 32 makes, without needing openssl.
        (umask 077 && od -An -tx1 -N32 /dev/urandom | tr -d ' \n' > "$home/mining.token")
        [ "$(wc -c < "$home/mining.token")" -eq 64 ] || fail "the mining token was not made"
        say "made a mining token, $home/mining.token: workers that have it may mine for your node"
    fi

    if ! cmp -s "$home/mining.token" "$root/docker/config/mining.token" 2>/dev/null; then
        (umask 077 && cp "$home/mining.token" "$root/docker/config/mining.token")
        say "copied it to docker/config/mining.token, for push to take to the VPS"
    fi
}

push() {
    target=
    key=${SSH_KEY:-}
    port=${SSH_PORT:-}

    while [ $# -gt 0 ]; do
        case "$1" in
            -i) [ $# -ge 2 ] || fail "-i takes the SSH private key's file"; key=$2; shift 2 ;;
            -p) [ $# -ge 2 ] || fail "-p takes the SSH port"; port=$2; shift 2 ;;
            -*) fail "push takes user@host, -i KEY and -p PORT, not $1" ;;
            *) target=$1; shift ;;
        esac
    done

    [ -n "$target" ] || fail "push to where? docker/pow.sh push user@host [-i KEY] [-p PORT]"
    [ -s "$root/docker/config/genesis.json" ] || fail "no docker/config/genesis.json: run docker/pow.sh init first"
    command -v rsync > /dev/null || fail "push needs rsync, here and on the VPS"
    dir=${REMOTE_DIR:-chainv4}

    # How rsync reaches the VPS: ssh, with the key and port given.
    ssh="ssh"

    if [ -n "$key" ]; then
        [ -r "$key" ] || fail "cannot read the SSH key $key"
        ssh="$ssh -i '$key' -o IdentitiesOnly=yes"
    fi

    if [ -n "$port" ]; then
        ssh="$ssh -p $port"
    fi

    # What the image builds from, and the chain's settings; no build output, no keys.
    say "copying the repository to $target:$dir"
    # The voting key goes, for the VPS to vote with; your own key never does.
    rsync -az -e "$ssh" \
        --include '/docker/config/validator.key' \
        --exclude 'target/' --exclude 'dist/' --exclude '.git/' --exclude '*.db/' \
        --exclude '*.key' --exclude '/docker/certs/*.pem' \
        "$root/" "$target:$dir/"
    say "next, on the VPS: $ssh $target, then cd $dir && docker/pow.sh up"
}

# Makes a file the node's alone, as its container sees it: uid 10001, read by nobody else. Rootless
# Podman maps that uid to one of the user's subordinate ids, which podman unshare reaches; Docker,
# and rootful Podman, need root. Without either, the file is left readable to everyone here.
node_owns() {
    engine=${CONTAINER_ENGINE:-$(sed -n 's/^CONTAINER_ENGINE=\([a-z]*\).*/\1/p' "$root/docker/.env" 2>/dev/null | tail -n 1)}
    if [ -z "$engine" ]; then
        if command -v docker > /dev/null; then engine=docker; else engine=podman; fi
    fi

    chmod 600 "$1"

    if [ "$engine" = podman ] && [ "$(podman info --format '{{.Host.Security.Rootless}}' 2> /dev/null)" = true ]; then
        podman unshare chown 10001:10001 "$1" && return
    elif [ "$(id -u)" = 0 ]; then
        chown 10001:10001 "$1" && return
    fi

    say "cannot give $1 to the node's user here: leaving it readable to every user of this machine"
    chmod 644 "$1"
}

up() {
    compose="$root/docker/compose.sh"
    [ -s "$root/docker/config/genesis.json" ] || fail "no docker/config/genesis.json: run docker/pow.sh init on your machine, then push"
    [ -s "$root/docker/.env" ] || fail "no docker/.env: run docker/pow.sh init on your machine, then push"

    # The node runs as uid 10001 in its container: the voting key and the token become its.
    for secret in "$root/docker/config/validator.key" "$root/docker/config/mining.token"; do
        if [ -s "$secret" ]; then node_owns "$secret"; fi
    done

    # The first build compiles RocksDB, Luau and both web apps: a machine with little memory runs
    # out without swap.
    memory=$(awk '/^MemTotal:/ { print int($2 / 1024) }' /proc/meminfo 2>/dev/null || echo 0)
    swap=$(awk '/^SwapTotal:/ { print int($2 / 1024) }' /proc/meminfo 2>/dev/null || echo 0)

    if [ "$memory" -gt 0 ] && [ $((memory + swap)) -lt 4000 ]; then
        say "this machine has ${memory} MiB of memory and ${swap} MiB of swap: the build wants about 4 GiB. Add swap if it fails"
    fi

    say "building and starting; the first build takes a while"
    "$compose" up -d --build

    say "waiting for the node"
    tries=0
    until "$compose" exec -T node curl -fs http://127.0.0.1:8080/status > /dev/null 2>&1; do
        tries=$((tries + 1))
        [ $tries -lt 60 ] || fail "the node did not come up: docker/compose.sh logs node"
        sleep 2
    done

    "$compose" exec -T node curl -fs http://127.0.0.1:8080/status | summary

    if command -v ufw > /dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then
        say "ufw is on: open the ports with  sudo ufw allow $p2p_port/udp && sudo ufw allow 80/tcp && sudo ufw allow 443/tcp"
    fi

    if [ -s "$root/docker/config/mining.token" ]; then
        say "it takes workers on TCP $mining_port: open that too, for docker/pow.sh worker"
    fi

    say "open UDP $p2p_port to the internet (and TCP 80 and 443 for the explorer and wallet), in the VPS's firewall too"
    say "next, on your machine: docker/pow.sh join <this VPS's address>"
}

join() {
    host=
    gpu=${GPU:-}

    while [ $# -gt 0 ]; do
        case "$1" in
            --gpu) gpu=1; shift ;;
            --cpu) gpu=0; shift ;;
            -*) fail "join takes a host, and --gpu or --cpu, not $1" ;;
            *) host=$1; shift ;;
        esac
    done

    [ -n "$host" ] || fail "join which VPS? docker/pow.sh join host [--gpu | --cpu]"
    [ -s "$home/genesis.json" ] || fail "no $home/genesis.json: run docker/pow.sh init first"
    address=$(owner)

    # Asked, in a terminal, when neither was given.
    if [ -z "$gpu" ] && [ -t 0 ]; then
        printf '[pow] mine on the GPU rather than %s CPU thread(s)? [y/N] ' "${MINE_THREADS:-1}" >&2
        read -r answer || answer=
        case "$answer" in y | Y | yes) gpu=1 ;; *) gpu=0 ;; esac
    fi

    case "${gpu:-0}" in
        1) chainv4=$(binary gpu); mining="--mine --mine-gpu"; how="on the GPU" ;;
        0) chainv4=$(binary); mining="--mine --mine-threads ${MINE_THREADS:-1}"; how="on ${MINE_THREADS:-1} CPU thread(s)" ;;
        *) fail "GPU is 1 or 0, not $gpu" ;;
    esac

    # QUIC dials an address, not a name: look the name up here.
    case "$host" in
        *:*) peer="/ip6/$host/udp/$p2p_port/quic-v1" ;;
        *[!0-9.]*)
            ip=$(python3 -I -c 'import socket, sys; print(socket.gethostbyname(sys.argv[1]))' "$host") || fail "cannot look up $host"
            peer="/ip4/$ip/udp/$p2p_port/quic-v1"
            ;;
        *) peer="/ip4/$host/udp/$p2p_port/quic-v1" ;;
    esac

    set -- --db "$home/chain.db" \
        --genesis "$home/genesis.json" \
        --peer "$peer" \
        --reward-to "$address" \
        --http "${API:-127.0.0.1:8090}" \
        --stream --index-events --index-addresses

    # The web apps, when they are built: trunk build --release in explorer/ and wallet/.
    if [ -s "$root/explorer/dist/index.html" ]; then set -- "$@" --explorer "$root/explorer/dist"; fi
    if [ -s "$root/wallet/dist/index.html" ]; then set -- "$@" --wallet "$root/wallet/dist"; fi

    if [ -n "${PROTOCOL:-}" ]; then set -- "$@" --protocol "$PROTOCOL"; fi

    say "mining here $how, rewards to $address, dialing the VPS at $peer"
    say "the API is at http://${API:-127.0.0.1:8090}; Ctrl-C stops the node, and join starts it again where it was"
    # Word-split on purpose: EXTRA_ARGS holds several arguments.
    # shellcheck disable=SC2086
    exec "$chainv4" "$@" $mining ${EXTRA_ARGS:-}
}

# A rig mining for the VPS's node, with no chain of its own: on the GPUs or threads here, with
# the mining token, until Ctrl-C.
worker() {
    host=
    gpu=${GPU:-}
    gpus=

    while [ $# -gt 0 ]; do
        case "$1" in
            --gpu) gpu=1; shift ;;
            --cpu) gpu=0; shift ;;
            --gpus) [ $# -ge 2 ] || fail "--gpus takes their numbers, such as 0,2"; gpu=1; gpus=$2; shift 2 ;;
            -*) fail "worker takes a host, and --gpu, --gpus or --cpu, not $1" ;;
            *) host=$1; shift ;;
        esac
    done

    [ -n "$host" ] || fail "mine for which node? docker/pow.sh worker host [--gpu [--gpus 0,2] | --cpu]"
    token=${TOKEN:-}

    if [ -z "$token" ]; then
        for candidate in "$home/mining.token" "$root/docker/config/mining.token"; do
            if [ -s "$candidate" ]; then token=$candidate; break; fi
        done
    fi

    [ -n "$token" ] && [ -s "$token" ] || fail "no mining token: copy the one docker/pow.sh token made to $home/mining.token, or give TOKEN"

    if [ -z "$gpu" ] && [ -t 0 ]; then
        printf '[pow] mine on the GPUs rather than %s CPU thread(s)? [y/N] ' "${MINE_THREADS:-1}" >&2
        read -r answer || answer=
        case "$answer" in y | Y | yes) gpu=1 ;; *) gpu=0 ;; esac
    fi

    case "${gpu:-0}" in
        1)
            chainv4=$(binary gpu)
            "$chainv4" gpus >&2 || fail "no GPU here: docker/pow.sh worker $host --cpu"
            set -- --gpu
            if [ -n "$gpus" ]; then set -- "$@" --gpus "$gpus"; fi
            ;;
        0) chainv4=$(binary); set -- --threads "${MINE_THREADS:-1}" ;;
        *) fail "GPU is 1 or 0, not $gpu" ;;
    esac

    if [ -n "${PROTOCOL:-}" ]; then set -- "$@" --protocol "$PROTOCOL"; fi

    case "$host" in
        *:*:*) node="[$host]:$mining_port" ;;
        *) node="$host:$mining_port" ;;
    esac

    say "mining for $node; the block rewards go to the node's REWARD_TO. Ctrl-C stops"
    exec "$chainv4" worker --node "$node" --token "$token" "$@"
}

logs() {
    exec "$root/docker/compose.sh" logs -f --tail "${LINES:-200}" node
}

# The chain, height and peers a node's /status and /node say.
summary() {
    python3 -I -c '
import json, sys
status = json.load(sys.stdin)
print("[pow] chain %s, height %s, difficulty %s, %s" % (status.get("genesis"), status.get("height"), status.get("difficulty"), status.get("coin", {}).get("symbol", "")))
'
}

status() {
    url=${1:-http://${API:-127.0.0.1:8090}}
    curl -fsk "$url/status" | summary
    curl -fsk "$url/node" | python3 -I -c '
import json, sys
node = json.load(sys.stdin)
peers = node.get("peers", [])
print("[pow] %d peer%s" % (len(peers), "" if len(peers) == 1 else "s"))
for peer in peers:
    print("[pow]   %s" % (peer.get("address") or peer.get("id") or peer))

def rate(hashes):
    for unit in ["H/s", "kH/s", "MH/s", "GH/s", "TH/s"]:
        if hashes < 1000 or unit == "TH/s":
            return "%.1f %s" % (hashes, unit)
        hashes /= 1000

mining = node.get("mining")
if mining:
    print("[pow] mining at %s%s" % (rate(mining["rate"]), ", workers on " + mining["listening"] if mining.get("listening") else ""))
    for device in mining["devices"]:
        print("[pow]   %s: %s" % (device["name"], rate(device["rate"])))
    for worker in mining["workers"]:
        print("[pow]   worker %s at %s: %s, %d found" % (worker["name"], worker["address"], rate(worker["rate"]), worker["found"]))
        for device in worker["devices"]:
            print("[pow]     %s: %s" % (device["name"], rate(device["rate"])))
'
}

command=${1:-}
[ $# -gt 0 ] && shift

case "$command" in
    init) init "$@" ;;
    push) push "$@" ;;
    up) up ;;
    join) join "$@" ;;
    token) token ;;
    worker) worker "$@" ;;
    status) status "$@" ;;
    logs) logs ;;
    *) sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
