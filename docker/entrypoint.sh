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
#   PROTOCOL         the name every protocol and topic starts with: chainv4 unless set
#   PEERS            multiaddrs to dial, separated by spaces
#   BOOTNODES        this node's own bootnodes, beside the chain's, separated by spaces
#   INDEX_EVENTS     which blocks the event index keeps: genesis (the default), a height,
#   INDEX_ADDRESSES  latest-N, or off; the same for the address index
#   FAUCET_KEY       a faucet's key file, for a dev or test chain: /config/faucet.key when
#                    there is one. With one, the API serves POST /faucet, paying from it
#   FAUCET_AMOUNT    each drip, in whole coins: 10 unless set
#   FAUCET_EVERY     how long an address, and an IP, wait between drips: 24h unless set
#   FAUCET_WORK      the zero bits a request's proof of work starts with: 20 unless set
#   SWAP_CONTRACT    the atomic swap contract deployed on this chain, contracts/lua/swap.lua,
#                    for GET /status to name to the wallet and chainv4 swap
#   DEX_CONTRACT     the DEX deployed on this chain, contracts/lua/dex.lua, for GET /status
#   DEX_ROUTER       the EVM DEX's router, DexRouter from contracts/evm/Dex.sol, 0x and 40 hex
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
    --index-addresses="${INDEX_ADDRESSES:-genesis}"

if [ -n "${PROTOCOL:-}" ]; then set -- "$@" --protocol "$PROTOCOL"; fi

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

        if [ -n "${MINING_TOKEN:-}" ] && [ ! -r "$MINING_TOKEN" ]; then
            echo "[docker] cannot read $MINING_TOKEN as uid $(id -u): taking no workers. Make it readable (docker/pow.sh up does)" >&2
        elif [ -n "${MINING_TOKEN:-}" ]; then
            set -- "$@" --mining 0.0.0.0:4002 --mining-token "$MINING_TOKEN"
            echo "[docker] taking workers on TCP 4002"
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

# Word-split on purpose: EXTRA_ARGS holds several arguments.
# shellcheck disable=SC2086
exec chainv4 "$@" ${EXTRA_ARGS:-}
