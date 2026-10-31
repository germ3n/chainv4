#!/bin/sh
# A proof-of-work chain across two machines: a VPS that runs the Docker stack with its QUIC port
# open to the internet, and this machine, which mines too and dials it. The VPS cannot dial back
# through this machine's NAT, but the one connection carries blocks both ways, and a node dials a
# --peer again every 15 seconds while it is cut off. Run from anywhere:
#
#   docker/pow.sh init                here, once: a key of your own, and a genesis that pays it,
#      [--finality ffg|authority|depth]  final by the VPS's votes (ffg, the default), or by depth;
#      [--governance council|electorate|none]  and who may change it later: you (council)
#   docker/pow.sh regenesis           here: a new chain for the same keys, with what this build's
#      [--finality …] [--governance …]  genesis has, such as names; the old genesis and this
#      [--yes]                          machine's chain move aside, after you say yes
#   docker/pow.sh push user@host      here: the repository, the genesis and .env, to the VPS;
#                  [-i KEY] [-p PORT]  -i the SSH private key to log in with, -p its port
#   docker/pow.sh up                  on the VPS, in that copy: build and start the stack, with
#                                     Docker or Podman
#   docker/pow.sh reset [--yes]       on the VPS: stop the node and move its chain and genesis
#                                     aside, in its volume, for a new genesis; up starts it afresh
#   docker/pow.sh join host           here: a node of the same chain that mines and dials the VPS,
#                  [--gpu | --cpu]     on the GPU or the CPU; asked when neither is given. Given the
#                                     VPS's onion, it goes through Tor alone, never showing its IP
#   docker/pow.sh token               here, once: the token workers mine for the VPS with; push
#                                     takes it there, and up starts taking workers
#   docker/pow.sh report-token        here, once: the token that shows you all the VPS's node keeps
#                                     from others, in the explorer's Settings and in status
#   docker/pow.sh faucet url          here: a faucet for the VPS's chain: a key of its own, funded
#                  [--fund COINS]      from your key through the node at url (1000 coins unless
#                                     said), for push to take there and up to run
#   docker/pow.sh worker host         any rig: mine for the VPS's node over its mining port, with
#      [--gpu [--gpus 0,2] | --cpu]    no chain of its own: every GPU, those picked, or threads
#   docker/pow.sh status [url]        either: the node's chain, height, peers and mining
#   docker/pow.sh logs                on the VPS: follow the node's log
#   docker/pow.sh onion               on the VPS: the node's onion address, with TOR=1 in .env
#
# The key stays here, in POW_HOME (~/.chainv4-pow): of what init makes, only the genesis goes to
# the VPS. Settings, all optional:
#
#   POW_HOME       where the key, genesis, this machine's chain and binary live
#   SYMBOL         the coin's ticker, for init: DEV unless set
#   CHAIN_NAME     the chain's name, for init and regenesis, as the apps show it: chainv4
#                  testnet unless set
#   NAMES          the suffix of the names init or regenesis gives the chain, such as dev for
#                  alice.dev: SYMBOL in lower case unless set; none for a chain without names
#   FINALITY       init's --finality; GOVERNANCE its --governance
#   SERVER_NAME    the VPS's domain or address, for init's .env and its certificate
#   MINE_THREADS   how many threads mine, here and on the VPS: 1 unless set
#   GPU            1 to mine on the GPU at join, 0 on the CPU, as --gpu and --cpu
#   P2P_PORT       the VPS's QUIC port: 4001 unless set
#   API            this machine's API, for join and status: 127.0.0.1:8090 unless set
#   REMOTE_DIR     where push puts the repository on the VPS: chainv4 under its home
#   SSH_KEY        push's SSH private key, as -i gives it; SSH_PORT its port, as -p
#   MINING_PORT    the VPS's mining port, for worker: 4002 unless set
#   PROXY          worker's SOCKS5 proxy, socks5h://host:port: for an .onion host, Tor's on
#                  127.0.0.1:9050 unless set
#   TOKEN          worker's token file: POW_HOME's mining.token, else docker/config's
#   PAYOUT         worker's address for a mining pool to pay its shares to, its claims kept in
#                  POW_HOME's pool-claims.json; alone, without a token, for an open pool
#   PROTOCOL       the chain's protocol name, for join and worker, as the VPS's .env has it
#   EXTRA_ARGS     anything else for join's node, such as --no-mdns
#   CONTAINER_ENGINE  docker or podman, for up: as docker/compose.sh picks it, unset
#   TOR=1          up runs tor too: the node's API, explorer and wallet as an onion service, and
#                  with TOR_MINING=1 its mining port; the onion's key lives in the tor volume
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

    make_genesis "$chainv4" "$finality" "$governance" "$address" "$voter"

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

# The development PoW chain with this finality and governance, and names; then its coins, the
# minting, the voters and the members, and the time it starts, are this key's and now. Written to
# POW_HOME and docker/config, for push.
make_genesis() {
    chainv4=$1 finality=$2 governance=$3 address=$4 voter=$5
    names=${NAMES:-$(printf %s "${SYMBOL:-DEV}" | tr '[:upper:]' '[:lower:]')}
    "$chainv4" genesis --consensus pow --finality "$finality" --governance "$governance" --fees base-fee --names "$names" --out "$home/genesis.json" > /dev/null
    python3 -I - "$home/genesis.json" "$address" "${SYMBOL:-DEV}" "$voter" "${CHAIN_NAME:-chainv4 testnet}" <<'EOF'
import json, sys, time
path, address, symbol, voter, name = sys.argv[1:]
with open(path) as f:
    genesis = json.load(f)
genesis["allocations"] = [{"address": address, "amount": str(10**6 * 10**genesis["coin"]["decimals"])}]
# Mint is for development chains alone, signed by the development key.
genesis["minter"] = None
genesis["coin"]["symbol"] = symbol
genesis["name"] = name
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
}

# Asks before what cannot be taken back, unless --yes said so already.
confirm() {
    [ "$1" = 1 ] && return
    [ -t 0 ] || fail "say --yes to go on without a terminal to ask in"
    printf '[pow] type yes to go on: ' >&2
    read -r answer || answer=
    [ "$answer" = yes ] || fail "nothing changed"
}

# A new chain for the keys init made: the same owner, voter and members, and what this build's
# genesis has, such as names and the EVM. The old genesis and this machine's chain move aside.
regenesis() {
    yes=0 finality= governance=

    while [ $# -gt 0 ]; do
        case "$1" in
            --finality) [ $# -ge 2 ] || fail "--finality takes ffg, authority or depth"; finality=$2; shift 2 ;;
            --governance) [ $# -ge 2 ] || fail "--governance takes council, electorate or none"; governance=$2; shift 2 ;;
            --yes) yes=1; shift ;;
            *) fail "regenesis takes --finality, --governance and --yes, not $1" ;;
        esac
    done

    address=$(owner)
    old="$root/docker/config/genesis.json"
    [ -s "$old" ] || fail "no $old to start again from: run docker/pow.sh init"
    command -v python3 > /dev/null || fail "regenesis needs python3, to read the old genesis"

    # The old chain's finality, governance and voting key, unless told otherwise.
    set -- $(python3 -I - "$old" <<'EOF'
import json, sys
genesis = json.load(open(sys.argv[1]))
finality = genesis.get("finality") or {}
voter = {"ffg": lambda: finality["validators"][0]["voter"], "authority": lambda: finality["signers"][0]["key"]}.get(finality.get("type"), lambda: "-")()
print(finality.get("type", "depth"), (genesis.get("governance") or {}).get("type", "none"), voter)
EOF
)
    finality=${finality:-$1} governance=${governance:-$2} voter=$3
    [ "$voter" = - ] && voter=
    [ "$finality" = depth ] || [ -n "$voter" ] || fail "the old genesis names no voter to keep"

    stamp=$(date +%Y%m%d-%H%M%S)
    say "a new chain for $address: $finality finality, $governance governance, names under ${NAMES:-$(printf %s "${SYMBOL:-DEV}" | tr '[:upper:]' '[:lower:]')}"
    say "moves aside, keeping them: $old, $home/genesis.json and $home/chain.db, each to .old-$stamp"
    say "the VPS needs docker/pow.sh reset before up, and everyone who joined needs the new genesis"
    confirm "$yes"

    mv "$old" "$old.old-$stamp"
    [ -e "$home/genesis.json" ] && mv "$home/genesis.json" "$home/genesis.json.old-$stamp"
    [ -e "$home/chain.db" ] && mv "$home/chain.db" "$home/chain.db.old-$stamp"

    make_genesis "$(binary)" "$finality" "$governance" "$address" "$voter"
    say "wrote the new genesis: next, docker/pow.sh push user@your-vps, then reset and up there"
}

# On the VPS: stops the node and moves its chain aside in its volume, for a new genesis, keeping it:
# the database, and the genesis the entrypoint copied there on the first start and keeps using.
reset() {
    yes=0
    [ "${1:-}" = --yes ] && yes=1
    compose="$root/docker/compose.sh"
    stamp=$(date +%Y%m%d-%H%M%S)

    say "stops the node and moves /data/chain.db and /data/genesis.json, in its volume, each to .old-$stamp"
    confirm "$yes"

    "$compose" stop node
    "$compose" run --rm --no-deps --entrypoint sh node -c "for kept in chain.db genesis.json; do [ ! -e /data/\$kept ] || mv /data/\$kept /data/\$kept.old-$stamp; done"
    say "moved; docker/pow.sh up starts the node on the new genesis"
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

# The token that shows its bearer all the node keeps from others: made once, kept in POW_HOME for
# status and the explorer here, and in docker/config for push to take to the VPS.
report_token() {
    if [ ! -s "$home/report.token" ]; then
        mkdir -p "$home"
        chmod 700 "$home"
        (umask 077 && od -An -tx1 -N32 /dev/urandom | tr -d ' \n' > "$home/report.token")
        [ "$(wc -c < "$home/report.token")" -eq 64 ] || fail "the report token was not made"
        say "made a report token, $home/report.token: paste it in the explorer's Settings to see all your node keeps from others"
    fi

    if ! cmp -s "$home/report.token" "$root/docker/config/report.token" 2>/dev/null; then
        (umask 077 && cp "$home/report.token" "$root/docker/config/report.token")
        say "copied it to docker/config/report.token, for push to take to the VPS"
    fi
}

# A faucet for the VPS's chain: a key of its own, which only ever holds what it is funded with,
# funded from the owner's key through a node of the chain, and the settings .env passes on.
faucet() {
    url=
    fund=1000

    while [ $# -gt 0 ]; do
        case "$1" in
            --fund) [ $# -ge 2 ] || fail "--fund takes a number of whole coins"; fund=$2; shift 2 ;;
            -*) fail "faucet takes a node's url and --fund COINS, not $1" ;;
            *) url=$1; shift ;;
        esac
    done

    [ -n "$url" ] || fail "fund it through which node? docker/pow.sh faucet https://your-vps [--fund COINS]"
    [ -s "$home/owner.key" ] || fail "no key yet: run docker/pow.sh init first"
    chainv4=$(binary)

    if [ ! -s "$home/faucet.key" ]; then
        (umask 077 && "$chainv4" keygen --out "$home/faucet.key" > "$home/faucet.address.out")
        sed -n 's/^address //p' "$home/faucet.address.out" > "$home/faucet.address"
        rm -f "$home/faucet.address.out"
        say "made the faucet's key, $home/faucet.key: it pays from its own account alone"
    fi

    (umask 077 && cp "$home/faucet.key" "$root/docker/config/faucet.key")
    address=$(cat "$home/faucet.address" 2> /dev/null)
    [ -n "$address" ] || fail "no address for the faucet's key in $home/faucet.address"

    # Whole coins, in the coin's smallest units, at the decimals the node says.
    units=$(curl -fsk "$url/status" | python3 -I -c '
import json, sys
from decimal import Decimal
decimals = json.load(sys.stdin)["coin"]["decimals"]
print(int(Decimal(sys.argv[1]) * 10 ** decimals))
' "$fund") || fail "could not read $url/status"

    say "funding the faucet, $address, with $fund coins from your key"
    "$chainv4" send --node "$url" --key "$home/owner.key" transfer --to "$address" --amount "$units"

    if ! grep -q '^FAUCET_AMOUNT=' "$root/docker/.env" 2>/dev/null; then
        {
            echo
            echo "# The faucet, docker/config/faucet.key, which docker/pow.sh faucet made and funded: each drip in"
            echo "# whole coins, how long an address and an IP wait between drips, and the proof of work's bits."
            echo "FAUCET_AMOUNT=10"
            echo "FAUCET_EVERY=24h"
            echo "FAUCET_WORK=20"
        } >> "$root/docker/.env"
    fi

    say "next: docker/pow.sh push to the VPS, then up there; top it up with docker/pow.sh faucet $url --fund COINS"
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
    # The voting key goes, for the VPS to vote with; your own key never does. Files gone here go
    # there too, as a module moved into a folder would otherwise be built twice; what is excluded,
    # the chain, keys, certificates and build output, rsync never deletes.
    rsync -az --delete -e "$ssh" \
        --include '/docker/config/validator.key' --include '/docker/config/faucet.key' \
        --exclude 'target/' --exclude 'dist/' --exclude '.git/' --exclude '*.db/' \
        --exclude '*.key' --exclude '/docker/certs/*.pem' --exclude '/validator-mnemonics.txt' \
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
    for secret in "$root/docker/config/validator.key" "$root/docker/config/mining.token" "$root/docker/config/report.token" "$root/docker/config/faucet.key"; do
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
    # Let's Encrypt's certbot and tor run as profiles of their own, when .env asks for them.
    # Recreated always: podman-compose keeps a running container on its old image after a build, so
    # an update would build and change nothing. The chain, certificates and onion key are in
    # volumes.
    set --
    if grep -q '^TLS_MODE=letsencrypt' "$root/docker/.env"; then set -- "$@" --profile letsencrypt; fi
    if grep -q '^TOR=1' "$root/docker/.env"; then set -- "$@" --profile tor; fi
    "$compose" "$@" up -d --build --force-recreate

    say "waiting for the node"
    tries=0
    until "$compose" exec -T node curl -fs http://127.0.0.1:8080/status > /dev/null 2>&1; do
        tries=$((tries + 1))
        [ $tries -lt 60 ] || fail "the node did not come up: docker/compose.sh logs node"
        sleep 2
    done

    "$compose" exec -T node curl -fs http://127.0.0.1:8080/status | summary

    # Peers' packets that came before the port mapping was up left UDP entries that skip it, which
    # their retries from the same port keep alive, so they never get through. The node just
    # started, so nothing is lost by clearing them.
    if command -v conntrack > /dev/null; then
        conntrack -D -p udp --orig-port-dst "$p2p_port" > /dev/null 2>&1 || true
    else
        say "no conntrack here: a peer that dialed before this node was up may stay unable to dial it. Install conntrack"
    fi

    if command -v ufw > /dev/null && ufw status 2>/dev/null | grep -q '^Status: active'; then
        # Published ports reach a rootful container by forwarding, so ufw allow, which rules what comes
        # to this machine itself, opens nothing for them. Its default forward policy stops them all.
        say "ufw is on: published ports need its route rules, not ufw allow: see Firewall in docs/docker.md"
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

    # QUIC dials an address, not a name: look the name up here. An onion is dialed through Tor
    # alone, the local Tor's unless PROXY names another: this node never shows its IP.
    tor=
    case "$host" in
        *.onion)
            peer="/onion3/${host%.onion}:$p2p_port"
            tor="--listen /ip4/127.0.0.1/tcp/0 --tor-proxy ${PROXY:-socks5h://127.0.0.1:9050} --tor-only --no-mdns"
            ;;
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
    exec "$chainv4" "$@" $mining $tor ${EXTRA_ARGS:-}
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

    # An open pool takes no token, only a payout address.
    if [ -z "$token" ] && [ -n "${PAYOUT:-}" ]; then
        token=open
    fi

    [ "$token" = open ] || { [ -n "$token" ] && [ -s "$token" ]; } || fail "no mining token: copy the one docker/pow.sh token made to $home/mining.token, or give TOKEN; for an open pool, give PAYOUT alone"

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

    # An onion is reached through Tor: the one running here, unless PROXY names another.
    case "$host" in
        *.onion) set -- "$@" --proxy "${PROXY:-socks5h://127.0.0.1:9050}" ;;
        *) if [ -n "${PROXY:-}" ]; then set -- "$@" --proxy "$PROXY"; fi ;;
    esac

    case "$host" in
        *:*:*) node="[$host]:$mining_port" ;;
        *) node="$host:$mining_port" ;;
    esac

    # A pool pays each worker's shares to the address it gives, and tells it each new claim on
    # them, kept in POW_HOME: chainv4 claim --saved claims by them should the pool's node be gone.
    if [ -n "${PAYOUT:-}" ]; then
        set -- "$@" --payout "$PAYOUT" --claims "$home/pool-claims.json"
        say "mining for $node; a pool's earnings go to $PAYOUT, claimed with chainv4 claim, its claims kept in $home/pool-claims.json. Ctrl-C stops"
    else
        say "mining for $node; the block rewards go to the node's REWARD_TO. Ctrl-C stops"
    fi

    if [ "$token" = open ]; then
        exec "$chainv4" worker --node "$node" "$@"
    fi

    exec "$chainv4" worker --node "$node" --token "$token" "$@"
}

# The node's onion address, once tor made it: on the VPS, where the stack runs with TOR=1.
onion() {
    cd "$root"
    compose=$root/docker/compose.sh
    address=$("$compose" --profile tor exec -T tor cat /var/lib/tor/chainv4/hostname 2>/dev/null) || fail "no onion yet: set TOR=1 in docker/.env, then docker/pow.sh up"
    say "the node's onion: http://$address"

    if grep -q '^TOR_MINING=1' "$root/docker/.env"; then
        say "workers mine through it: chainv4 worker --node $address:$mining_port --proxy socks5h://127.0.0.1:9050 …"
    fi
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
    # Your own node, from elsewhere, shows you all it keeps from others with its report token.
    set --
    case "$url" in
        https://* | *.onion* | http://127.* | http://localhost*)
            if [ -s "$home/report.token" ]; then set -- -H "Authorization: Bearer $(cat "$home/report.token")"; fi
            ;;
    esac
    curl -fsk "$@" "$url/node" | python3 -I -c '
import json, sys
node = json.load(sys.stdin)
peers = node.get("peers", [])
hidden = node.get("hidden", [])
if hidden:
    print("[pow] the node keeps to itself: %s" % ", ".join(hidden))
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
        print("[pow]   worker %s at %s: %s, %d found" % (worker["name"], worker["address"] or "a hidden address", rate(worker["rate"]), worker["found"]))
        for device in worker["devices"]:
            print("[pow]     %s: %s" % (device["name"], rate(device["rate"])))
'
}

command=${1:-}
[ $# -gt 0 ] && shift

case "$command" in
    init) init "$@" ;;
    regenesis) regenesis "$@" ;;
    push) push "$@" ;;
    up) up ;;
    reset) reset "$@" ;;
    join) join "$@" ;;
    token) token ;;
    report-token) report_token ;;
    faucet) faucet "$@" ;;
    worker) worker "$@" ;;
    status) status "$@" ;;
    logs) logs ;;
    onion) onion ;;
    *) sed -n '2,56p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
