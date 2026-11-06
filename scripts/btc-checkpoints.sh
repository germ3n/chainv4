#!/bin/sh
# Refreshes the wallet's built-in Bitcoin checkpoints (wallet/src/light.rs, Net::checkpoint), the
# blocks a first sync of its light client begins at, before each release: so a new install scans a
# week or so, not months. For each network it takes the block 1,008 below the tip (about a week),
# and its hash from two block explorers where there are two, refusing to write one they disagree
# on. Run it, look at the diff, and commit it with the release.
#
#   scripts/btc-checkpoints.sh [--check]
#
# --check only says whether the checkpoints in the source are more than 8 weeks old.
set -eu

cd "$(dirname "$0")/.."
FILE=wallet/src/light.rs
DEPTH=1008
CHECK=${1:-}

# network, the name in light.rs, then the explorers' API roots, the first leading
set -- \
    "Main|https://mempool.space/api|https://blockstream.info/api" \
    "Testnet4|https://mempool.space/testnet4/api|" \
    "Signet|https://mempool.space/signet/api|https://blockstream.info/signet/api"

for entry in "$@"; do
    net=${entry%%|*}
    rest=${entry#*|}
    first=${rest%%|*}
    second=${rest#*|}
    tip=$(curl -fsS -m 20 "$first/blocks/tip/height")
    written=$(grep -E "Net::$net => at\(" "$FILE" | sed -E 's/.*at\(([0-9_]+),.*/\1/' | tr -d _)

    if [ "$CHECK" = "--check" ]; then
        age=$((tip - written))
        weeks=$((age / 1008))
        echo "$net: checkpoint at $written, $age blocks (about $weeks weeks) below the tip $tip"
        [ "$weeks" -le 8 ] || echo "  older than 8 weeks: run scripts/btc-checkpoints.sh"
        continue
    fi

    height=$((tip - DEPTH))
    hash=$(curl -fsS -m 20 "$first/block-height/$height")

    if [ -n "$second" ]; then
        other=$(curl -fsS -m 20 "$second/block-height/$height")

        if [ "$hash" != "$other" ]; then
            echo "$net: the explorers disagree on block $height ($hash, $other): nothing written" >&2
            exit 1
        fi
    fi

    case "$hash" in
        *[!0-9a-f]* | "") echo "$net: $hash is no block hash" >&2; exit 1 ;;
    esac

    if [ "${#hash}" -ne 64 ]; then
        echo "$net: $hash is no block hash" >&2
        exit 1
    fi

    # Grouped by thousands, as the source writes numbers: 973_809.
    pretty=$(printf "%d" "$height" | rev | sed -E 's/([0-9]{3})/\1_/g' | rev | sed 's/^_//')
    sed -i.bak -E "s/(Net::$net => at\()[0-9_]+, \"[0-9a-f]{64}\"/\1$pretty, \"$hash\"/" "$FILE"
    rm -f "$FILE.bak"
    echo "$net: block $pretty, $hash${second:+, checked with a second explorer}"
done
