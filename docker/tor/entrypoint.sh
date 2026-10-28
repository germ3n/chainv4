#!/bin/sh
# Writes tor's configuration and runs it: one onion service, the node's HTTP API on its port 80,
# its peer-to-peer over TCP on P2P_PORT, and with TOR_MINING=1 its mining port, offered on the
# number its workers use. The SOCKS port takes compose's network alone, for the node to dial onion
# peers through. The onion's address is printed once tor made it, and written for the node as a
# multiaddr, /onion3/…:P2P_PORT, in the onion volume; docker/pow.sh onion prints it again.
set -eu

data=/var/lib/tor
service="$data/chainv4"
mining_port=${MINING_PORT:-4002}
p2p_port=${P2P_PORT:-4001}
mkdir -p "$service"
chmod 700 "$data" "$service"

{
    echo "DataDirectory $data"
    echo "SocksPort 0.0.0.0:9050"
    echo "SocksPolicy accept 10.0.0.0/8"
    echo "SocksPolicy accept 172.16.0.0/12"
    echo "SocksPolicy accept 192.168.0.0/16"
    echo "SocksPolicy reject *"
    echo "HiddenServiceDir $service"
    echo "HiddenServiceVersion 3"
    echo "HiddenServicePort 80 node:8080"
    echo "HiddenServicePort $p2p_port node:$p2p_port"

    if [ "${TOR_MINING:-0}" = 1 ]; then
        echo "HiddenServicePort $mining_port node:4002"
    fi
} > /tmp/torrc

# The address, once tor wrote it: on the first start, a moment after it begins.
(
    tries=0
    while [ ! -s "$service/hostname" ] && [ $tries -lt 120 ]; do
        sleep 1
        tries=$((tries + 1))
    done

    if [ -s "$service/hostname" ]; then
        onion=$(cat "$service/hostname")
        echo "[tor] the node's onion: http://$onion"
        printf '/onion3/%s:%s\n' "${onion%.onion}" "$p2p_port" > /onion/p2p
        chmod 644 /onion/p2p
    fi
) &

exec tor -f /tmp/torrc
