#!/bin/sh
# Starts a chainv4 node in its container, with the API on :8080 for nginx in front, the explorer at
# / and the wallet at /wallet/, both indexes, and peer-to-peer over QUIC on P2P_PORT, and with
# TOR=1 over TCP too, for its onion.
#
# Its chain is /data/genesis.json, written on the first start and kept, so the chain stays the same
# across restarts: copied from /config/genesis.json (docker/config/genesis.json) when there is one,
# or else made from CHAIN_CONSENSUS (pow, authority), CHAIN_FINALITY (ffg, authority, depth, none)
# and CHAIN_FEES (base-fee, bid, free). Everything else follows from the genesis, unless set:
#
#   BLOCKS           auto (the default): mine under proof of work, produce under an authority;
#                    or mine, produce, off
#   VALIDATE         auto (the default): vote under FFG and authority finality; or on, off
#   PRODUCER_KEY     the authority's key file: /config/producer.key when there is one, else the
#                    development key
#   VALIDATOR_KEY    the voting key file: /config/validator.key when there is one, else the
#                    development key
#   REWARD_TO        where mining rewards go
#   MINE_THREADS     how many threads mine, 1 by default; 0 for none, when workers mine
#   MINING_TOKEN     the token workers must know: /config/mining.token when there is one. With
#                    one, a mining node takes workers (chainv4 worker) on TCP 4002
#   POOL             1 runs a mining pool on that port: workers' shares, and every block sends
#                    their share to the pool's escrow, which they claim from
#   POOL_OPEN        1 runs an open pool, which anyone joins with a payout address, no token
#   MINING_SOLO      1 takes workers mining alone on the port too: each block one finds pays it
#   MINING_OPEN      1 opens the port to anyone with a payout address, no token: with POOL or
#                    MINING_SOLO, which the node needs beside it
#   MINING_PUBLIC    where workers reach the port from outside, host:port, as GET /mining says
#   TOR_MINING       1, with TOR=1, offers the port at the onion too, on MINING_PORT: GET /mining
#                    names it as soon as tor has made the onion, for apps that reach the node there
#   REPORT           what the node shares of itself with whoever asks: public unless set, or
#                    full or minimal
#   HIDE, SHOW       fields to keep or share on top of it, separated by commas
#   REPORT_TOKEN     the token that shows everything: /config/report.token when there is one
#   POOL_FEE         what the pool keeps of each block, in percent: 0 unless set
#   POOL_WINDOW      how many blocks' worth of shares each block shares out by: 2 unless set
#   POOL_PUSH        claim for miners owed at least this, in the coin's smallest unit, from
#   POOL_PUSH_KEY    this key file, which pays the fees: /config/pool-push.key when there is one
#   PROTOCOL         the name every protocol and topic starts with: chainv4 unless set
#   CHAIN_NAME       the chain's name in a preset's genesis, as the apps show it: none unless
#                    set, which the apps show as chainv4
#   UPDATES          where the node looks for releases, a manifest's URL, or off: the release
#                    keys' own unless set. The image never updates itself: pull a new one
#   PEERS            multiaddrs to dial, separated by spaces
#   BOOTNODES        this node's own bootnodes, beside the chain's, separated by spaces
#   INDEX_EVENTS     which blocks the event index keeps: genesis (the default), a height,
#   INDEX_ADDRESSES  latest-N, or off; the same for the address index
#   INDEX_MULTISIGS  on (the default) or off: the index of multisigs by their members
#   FAUCET_KEY       a faucet's key file, for a dev or test chain: /config/faucet.key when
#                    there is one. With one, the API serves POST /faucet, paying from it
#   FAUCET_AMOUNT    each drip, in whole coins: 10 unless set
#   FAUCET_EVERY     how long an address, and an IP, wait between drips: 24h unless set
#   FAUCET_WORK      the zero bits a request's proof of work starts with: 20 unless set
#   SWAP_CONTRACT    the atomic swap contract deployed on this chain, contracts/lua/swap.lua,
#                    for GET /status to name to the wallet and chainv4 swap
#   DEX_CONTRACT     the DEX deployed on this chain, contracts/lua/dex.lua, for GET /status
#   DEX_ROUTER       the EVM DEX's router, DexRouter from contracts/evm/Dex.sol, 0x and 40 hex
#   TRUSTED_PROXY    the proxies whose X-Forwarded-For the API believes: their names or addresses
#   MEMPOOL_SIZE, MEMPOOL_PER_SENDER, MAX_NONCE_GAP, REPLACE_BUMP_PERCENT, UNDERPRICED_FOR,
#   SYNC_BATCH_BYTES, MAX_PEERS_IN, BAN_SCORE, BAN_WINDOW, MAX_STRIKES, STRIKE_WINDOW,
#   TOR_TIMEOUT, HTTP_RATE, API_MAX_PAGE, SUGGEST_MULTIPLIER, UPDATE_EVERY, LUA_STATES,
#   LUA_BUILDERS, WASM_CACHE_MB, WASM_CACHE_MODULES and TUNE
#                    the node's limits, each the flag of its name (TUNE is --tune KEY=VALUE,...),
#                    set only when given: unset, each keeps chainv4's default (docs/node.md)
#   EXTRA_ARGS       anything else, passed on to chainv4
set -eu

data=/data
config=/config
genesis=$data/genesis.json
consensus=${CHAIN_CONSENSUS:-authority}
finality=${CHAIN_FINALITY:-ffg}
fees=${CHAIN_FEES:-base-fee}

# Files in docker/config: a genesis to start the chain from, and the keys to sign with.
if [ ! -s "$genesis" ] && [ -s "$config/genesis.json" ]; then
    cp "$config/genesis.json" "$genesis"
    echo "[docker] starting the chain $config/genesis.json describes"
fi

if [ -z "${PRODUCER_KEY:-}" ] && [ -s "$config/producer.key" ]; then
    PRODUCER_KEY=$config/producer.key
fi

if [ -z "${VALIDATOR_KEY:-}" ] && [ -s "$config/validator.key" ]; then
    VALIDATOR_KEY=$config/validator.key
fi

if [ -z "${MINING_TOKEN:-}" ] && [ -s "$config/mining.token" ]; then
    MINING_TOKEN=$config/mining.token
fi

if [ ! -s "$genesis" ]; then
    chainv4 genesis --consensus "$consensus" --finality "$finality" --fees "$fees" --out "$genesis"
    if [ -n "${CHAIN_NAME:-}" ]; then jq --arg name "$CHAIN_NAME" '.name = $name' "$genesis" > "$genesis.new" && mv "$genesis.new" "$genesis"; fi
    echo "[docker] wrote $genesis: $consensus blocks, $finality finality, $fees fees"
fi

# What the genesis says, whoever wrote it.
consensus=$(jq -r '.consensus.type' "$genesis")
finality=$(jq -r '.finality.type // "none"' "$genesis")
echo "[docker] chain $genesis: $consensus blocks, $finality finality"

set -- --db "$data/chain.db" \
    --genesis "$genesis" \
    --http 0.0.0.0:8080 \
    --listen "/ip4/0.0.0.0/udp/${P2P_PORT:-4001}/quic-v1" \
    --stream \
    --explorer /opt/chainv4/explorer \
    --wallet /opt/chainv4/wallet \
    --index-events="${INDEX_EVENTS:-genesis}" \
    --index-addresses="${INDEX_ADDRESSES:-genesis}" \
    --index-multisigs="${INDEX_MULTISIGS:-on}"

if [ -n "${PROTOCOL:-}" ]; then set -- "$@" --protocol "$PROTOCOL"; fi

# What it shares of itself with whoever asks, and the token that shows its bearer everything.
if [ -z "${REPORT_TOKEN:-}" ] && [ -s "$config/report.token" ]; then REPORT_TOKEN=$config/report.token; fi
if [ -n "${REPORT:-}" ]; then set -- "$@" --report "$REPORT"; fi
if [ -n "${HIDE:-}" ]; then set -- "$@" --hide "$HIDE"; fi
if [ -n "${SHOW:-}" ]; then set -- "$@" --show "$SHOW"; fi

if [ -n "${REPORT_TOKEN:-}" ]; then
    if [ -r "$REPORT_TOKEN" ]; then
        set -- "$@" --report-token "$REPORT_TOKEN"
    else
        echo "[docker] cannot read $REPORT_TOKEN as uid $(id -u): no report token. Make it readable (docker/pow.sh up does)" >&2
    fi
fi
if [ -n "${UPDATES:-}" ]; then set -- "$@" --updates "$UPDATES"; fi

blocks=${BLOCKS:-auto}

if [ "$blocks" = auto ]; then
    case "$consensus" in
        pow) blocks=mine ;;
        authority) blocks=produce ;;
        *) blocks=off ;;
    esac
fi

development=

case "$blocks" in
    mine)
        set -- "$@" --mine --mine-threads "${MINE_THREADS:-1}"
        if [ -n "${REWARD_TO:-}" ]; then set -- "$@" --reward-to "$REWARD_TO"; fi

        pool=${POOL:-0}

        port=1

        if [ "${POOL_OPEN:-0}" = 1 ]; then
            set -- "$@" --mining 0.0.0.0:4002 --mining-open
            pool=1
            echo "[docker] an open mining pool on TCP 4002: anyone may join with a payout address"
        elif [ "${MINING_OPEN:-0}" = 1 ]; then
            set -- "$@" --mining 0.0.0.0:4002 --mining-open
            echo "[docker] an open mining port on TCP 4002: anyone may mine with a payout address"
        elif [ -n "${MINING_TOKEN:-}" ] && [ ! -r "$MINING_TOKEN" ]; then
            echo "[docker] cannot read $MINING_TOKEN as uid $(id -u): taking no workers. Make it readable (docker/pow.sh up does)" >&2
            pool=0
        elif [ -n "${MINING_TOKEN:-}" ]; then
            set -- "$@" --mining 0.0.0.0:4002 --mining-token "$MINING_TOKEN"
            echo "[docker] taking workers on TCP 4002"
        else
            pool=0
            port=0
        fi

        if [ "$port" = 1 ] && [ "${MINING_SOLO:-0}" = 1 ]; then
            set -- "$@" --mining-solo
            echo "[docker] taking workers mining alone, each block one finds paying it in full"
        fi

        if [ "$port" = 1 ] && [ -n "${MINING_PUBLIC:-}" ]; then set -- "$@" --mining-public "$MINING_PUBLIC"; fi
        if [ "$port" = 1 ] && [ "${TOR:-0}" = 1 ] && [ "${TOR_MINING:-0}" = 1 ]; then set -- "$@" --mining-onion-port "${MINING_PORT:-4002}"; fi

        if [ "$pool" = 1 ]; then
            set -- "$@" --pool --pool-fee "${POOL_FEE:-0}" --pool-window "${POOL_WINDOW:-2}"

            if [ -z "${POOL_PUSH_KEY:-}" ] && [ -s "$config/pool-push.key" ]; then POOL_PUSH_KEY=$config/pool-push.key; fi
            if [ -n "${POOL_PUSH:-}" ] && [ -n "${POOL_PUSH_KEY:-}" ]; then set -- "$@" --pool-push "$POOL_PUSH" --pool-push-key "$POOL_PUSH_KEY"; fi
            echo "[docker] running a mining pool: a fee of ${POOL_FEE:-0}%, shares over ${POOL_WINDOW:-2} blocks"
        fi
        ;;
    produce)
        set -- "$@" --produce
        if [ -n "${PRODUCER_KEY:-}" ]; then set -- "$@" --producer-key "$PRODUCER_KEY"; else development=1; fi
        ;;
    off) ;;
    *) echo "[docker] BLOCKS is auto, mine, produce or off, not $blocks" >&2; exit 1 ;;
esac

validate=${VALIDATE:-auto}

if [ "$validate" = auto ]; then
    case "$finality" in
        ffg | authority) validate=on ;;
        *) validate=off ;;
    esac
fi

case "$validate" in
    on)
        set -- "$@" --validate
        if [ -n "${VALIDATOR_KEY:-}" ]; then set -- "$@" --validator-key "$VALIDATOR_KEY"; else development=1; fi
        ;;
    off) ;;
    *) echo "[docker] VALIDATE is auto, on or off, not $validate" >&2; exit 1 ;;
esac

if [ -n "$development" ]; then
    echo "[docker] signing with the development key, whose seed is public: fine for a dev chain, never for one of value" >&2
fi

for peer in ${PEERS:-}; do
    set -- "$@" --peer "$peer"
done

for bootnode in ${BOOTNODES:-}; do
    set -- "$@" --bootnode "$bootnode"
done

# The node runs as uid 10001: a key it cannot read would stop it, so say why first.
for key in "${PRODUCER_KEY:-}" "${VALIDATOR_KEY:-}"; do
    if [ -n "$key" ] && [ ! -r "$key" ]; then
        echo "[docker] cannot read $key as uid $(id -u): give it to that user (docker/pow.sh up does)" >&2
        exit 1
    fi
done

# A faucet, when there is a key for one.
FAUCET_KEY=${FAUCET_KEY:-}
if [ -z "$FAUCET_KEY" ] && [ -s "$config/faucet.key" ]; then FAUCET_KEY=$config/faucet.key; fi

if [ -n "$FAUCET_KEY" ] && [ ! -r "$FAUCET_KEY" ]; then
    echo "[docker] cannot read $FAUCET_KEY as uid $(id -u): running no faucet. Make it readable (docker/pow.sh up does)" >&2
elif [ -n "$FAUCET_KEY" ]; then
    set -- "$@" --faucet-key "$FAUCET_KEY" --faucet-amount "${FAUCET_AMOUNT:-10}" --faucet-every "${FAUCET_EVERY:-24h}" --faucet-work "${FAUCET_WORK:-20}"
    echo "[docker] running a faucet: ${FAUCET_AMOUNT:-10} a drip, every ${FAUCET_EVERY:-24h}"
fi

# The swap contract, when this chain has one deployed.
if [ -n "${SWAP_CONTRACT:-}" ]; then
    set -- "$@" --swap-contract "$SWAP_CONTRACT"
    echo "[docker] naming the swap contract $SWAP_CONTRACT"
fi

# The DEXes, when this chain has them deployed.
if [ -n "${DEX_CONTRACT:-}" ]; then
    set -- "$@" --dex-contract "$DEX_CONTRACT"
    echo "[docker] naming the DEX $DEX_CONTRACT"
fi

if [ -n "${DEX_ROUTER:-}" ]; then
    set -- "$@" --dex-router "$DEX_ROUTER"
    echo "[docker] naming the EVM DEX's router $DEX_ROUTER"
fi

# With TOR=1, peer-to-peer over Tor too: TCP beside QUIC, which tor publishes in the node's onion,
# onion peers dialed through tor's SOCKS port on compose's network, and the onion's address read
# from the file tor writes, to tell peers.
if [ "${TOR:-0}" = 1 ]; then
    set -- "$@" --listen "/ip4/0.0.0.0/tcp/${P2P_PORT:-4001}" --tor-proxy socks5h://tor:9050 --onion-file /onion/p2p
    echo "[docker] peer-to-peer over Tor too: TCP ${P2P_PORT:-4001} for the onion, onions dialed through tor"
fi

# The node's limits, each passed on only when set, so one left unset keeps chainv4's own default.
for pair in \
    MEMPOOL_SIZE:--mempool-size \
    MEMPOOL_PER_SENDER:--mempool-per-sender \
    MAX_NONCE_GAP:--max-nonce-gap \
    REPLACE_BUMP_PERCENT:--replace-bump-percent \
    UNDERPRICED_FOR:--underpriced-for \
    SYNC_BATCH_BYTES:--sync-batch-bytes \
    MAX_PEERS_IN:--max-peers-in \
    BAN_SCORE:--ban-score \
    BAN_WINDOW:--ban-window \
    MAX_STRIKES:--max-strikes \
    STRIKE_WINDOW:--strike-window \
    TOR_TIMEOUT:--tor-timeout \
    HTTP_RATE:--http-rate \
    API_MAX_PAGE:--api-max-page \
    SUGGEST_MULTIPLIER:--suggest-multiplier \
    UPDATE_EVERY:--update-every \
    LUA_STATES:--lua-states \
    LUA_BUILDERS:--lua-builders \
    WASM_CACHE_MB:--wasm-cache-mb \
    WASM_CACHE_MODULES:--wasm-cache-modules \
    TUNE:--tune; do
    name=${pair%%:*}
    flag=${pair#*:}
    eval "value=\${$name:-}"
    if [ -n "$value" ]; then set -- "$@" "$flag" "$value"; fi
done

# The proxies whose X-Forwarded-For the API believes, by name, as a container's address can change:
# nginx in this stack, without which every visitor counts as one against the per-IP limits.
for proxy in ${TRUSTED_PROXY:-}; do
    set -- "$@" --trusted-proxy "$proxy"
done

# Word-split on purpose: EXTRA_ARGS holds several arguments.
# shellcheck disable=SC2086
exec chainv4 "$@" ${EXTRA_ARGS:-}
