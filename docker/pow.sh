#!/bin/sh
# A proof-of-work chain across two machines: a VPS that runs the Docker stack with its QUIC port
# open to the internet, and this machine, which mines too and dials it. The VPS cannot dial back
# through this machine's NAT, but the one connection carries blocks both ways, and a node dials a
# --peer again every 15 seconds while it is cut off. Run from anywhere:
#
#   docker/pow.sh init                here, once: a key of your own, and a genesis that pays it
#   docker/pow.sh push user@host      here: the repository, the genesis and .env, to the VPS;
#                  [-i KEY] [-p PORT]  -i the SSH private key to log in with, -p its port
#   docker/pow.sh up                  on the VPS, in that copy: build and start the stack, with
#                                     Docker or Podman
#   docker/pow.sh join host           here: a node of the same chain that mines and dials the VPS
#   docker/pow.sh status [url]        either: the node's chain, height and peers
#
# The key stays here, in POW_HOME (~/.chainv4-pow): of what init makes, only the genesis goes to
# the VPS. Settings, all optional:
#
#   POW_HOME       where the key, genesis, this machine's chain and binary live
#   SYMBOL         the coin's ticker, for init: DEV unless set
#   SERVER_NAME    the VPS's domain or address, for init's .env and its certificate
#   MINE_THREADS   how many threads mine, here and on the VPS: 1 unless set
#   P2P_PORT       the VPS's QUIC port: 4001 unless set
#   API            this machine's API, for join and status: 127.0.0.1:8090 unless set
#   REMOTE_DIR     where push puts the repository on the VPS: chainv4 under its home
#   SSH_KEY        push's SSH private key, as -i gives it; SSH_PORT its port, as -p
#   EXTRA_ARGS     anything else for join's node, such as --no-mdns
#   CONTAINER_ENGINE  docker or podman, for up: as docker/compose.sh picks it, unset
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
home=${POW_HOME:-$HOME/.chainv4-pow}
p2p_port=${P2P_PORT:-4001}

say() { printf '[pow] %s\n' "$*" >&2; }
fail() { printf '[pow] %s\n' "$*" >&2; exit 1; }

# The release node, built here and kept in POW_HOME: an editor's cargo check may clear target/,
# and a running node keeps its binary anyway. Copied in by renaming, as macOS kills a binary run
# again after it was overwritten where it lay.
binary() {
    if [ ! -x "$home/bin/chainv4" ] || [ "$root/target/release/chainv4" -nt "$home/bin/chainv4" ] || [ "${REBUILD:-}" = 1 ]; then
        say "building the node"
        (cd "$root" && cargo build --release --locked --quiet)
        mkdir -p "$home/bin"
        cp "$root/target/release/chainv4" "$home/bin/chainv4.new"
        mv -f "$home/bin/chainv4.new" "$home/bin/chainv4"
    fi

    echo "$home/bin/chainv4"
}

owner() {
    [ -s "$home/owner.address" ] || fail "no key yet: run docker/pow.sh init first"
    cat "$home/owner.address"
}

init() {
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

    # The development PoW chain, with depth finality, nothing to vote; then its coins, the minting
    # and the time it starts are this key's and now.
    "$chainv4" genesis --consensus pow --finality depth --fees base-fee --out "$home/genesis.json" > /dev/null
    python3 -I - "$home/genesis.json" "$address" "${SYMBOL:-DEV}" <<'EOF'
import json, sys, time
path, address, symbol = sys.argv[1:]
with open(path) as f:
    genesis = json.load(f)
genesis["allocations"] = [{"address": address, "amount": str(10**6 * 10**genesis["coin"]["decimals"])}]
genesis["minter"] = address
genesis["coin"]["symbol"] = symbol
genesis["timestamp"] = int(time.time() * 1000)
with open(path, "w") as f:
    json.dump(genesis, f, indent=2)
    f.write("\n")
EOF
    cp "$home/genesis.json" "$root/docker/config/genesis.json"

    if [ -e "$root/docker/.env" ]; then
        say "docker/.env is there already: set REWARD_TO=$address in it"
    else
        cat > "$root/docker/.env" <<EOF
# Written by docker/pow.sh init. The chain is docker/config/genesis.json: a proof-of-work chain
# with depth finality, so the node mines and never votes.
# docker or podman, as docker/compose.sh runs it; unset, whichever the VPS has.
CONTAINER_ENGINE=${CONTAINER_ENGINE:-}
REWARD_TO=$address
MINE_THREADS=${MINE_THREADS:-1}
PEERS=
INDEX_EVENTS=genesis
INDEX_ADDRESSES=genesis
# A VPS's network holds none of your nodes: peers dial it.
EXTRA_ARGS=--no-mdns

# HTTPS in front: self-signed until you give the VPS a domain and use letsencrypt.
TLS_MODE=self-signed
SERVER_NAME=${SERVER_NAME:-localhost}
HTTP_PORT=80
HTTPS_PORT=443
P2P_PORT=$p2p_port
EOF
    fi

    say "made $home: the key (owner.key, which never leaves this machine) and the genesis"
    say "coins and mining rewards go to $address"
    say "next: docker/pow.sh push user@your-vps"
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
    rsync -az -e "$ssh" \
        --exclude 'target/' --exclude 'dist/' --exclude '.git/' --exclude '*.db/' \
        --exclude '*.key' --exclude '/docker/certs/*.pem' \
        "$root/" "$target:$dir/"
    say "next, on the VPS: $ssh $target, then cd $dir && docker/pow.sh up"
}

up() {
    compose="$root/docker/compose.sh"
    [ -s "$root/docker/config/genesis.json" ] || fail "no docker/config/genesis.json: run docker/pow.sh init on your machine, then push"
    [ -s "$root/docker/.env" ] || fail "no docker/.env: run docker/pow.sh init on your machine, then push"

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

    say "open UDP $p2p_port to the internet (and TCP 80 and 443 for the explorer and wallet), in the VPS's firewall too"
    say "next, on your machine: docker/pow.sh join <this VPS's address>"
}

join() {
    host=${1:-}
    [ -n "$host" ] || fail "join which VPS? docker/pow.sh join host"
    [ -s "$home/genesis.json" ] || fail "no $home/genesis.json: run docker/pow.sh init first"
    address=$(owner)
    chainv4=$(binary)

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
        --mine --mine-threads "${MINE_THREADS:-1}" --reward-to "$address" \
        --http "${API:-127.0.0.1:8090}" \
        --stream --index-events --index-addresses

    # The web apps, when they are built: trunk build --release in explorer/ and wallet/.
    if [ -s "$root/explorer/dist/index.html" ]; then set -- "$@" --explorer "$root/explorer/dist"; fi
    if [ -s "$root/wallet/dist/index.html" ]; then set -- "$@" --wallet "$root/wallet/dist"; fi

    say "mining here, rewards to $address, dialing the VPS at $peer"
    say "the API is at http://${API:-127.0.0.1:8090}; Ctrl-C stops the node, and join starts it again where it was"
    # Word-split on purpose: EXTRA_ARGS holds several arguments.
    # shellcheck disable=SC2086
    exec "$chainv4" "$@" ${EXTRA_ARGS:-}
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
'
}

command=${1:-}
[ $# -gt 0 ] && shift

case "$command" in
    init) init ;;
    push) push "$@" ;;
    up) up ;;
    join) join "$@" ;;
    status) status "$@" ;;
    *) sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
