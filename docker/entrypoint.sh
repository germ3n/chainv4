#!/bin/sh
# Starts a chainv4 node in its container, with the API on :8080 for nginx in front, the explorer at
# / and the wallet at /wallet/, both indexes, and peer-to-peer over QUIC on P2P_PORT.
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
#   MINE_THREADS     how many threads mine, 1 by default
#   PEERS            multiaddrs to dial, separated by spaces
#   INDEX_EVENTS     which blocks the event index keeps: genesis (the default), a height,
#   INDEX_ADDRESSES  latest-N, or off; the same for the address index
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

# Word-split on purpose: EXTRA_ARGS holds several arguments.
# shellcheck disable=SC2086
exec chainv4 "$@" ${EXTRA_ARGS:-}
