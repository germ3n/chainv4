#!/bin/sh
# Runs before nginx starts, from the nginx image's /docker-entrypoint.d. It renders the site's
# config, and puts a certificate where nginx reads it, /etc/chainv4/tls, by TLS_MODE:
#
#   self-signed  one made here for SERVER_NAME on the first start, and kept in the tls volume
#   provided     yours: fullchain.pem and privkey.pem in /etc/chainv4/provided (docker/certs)
#   letsencrypt  certbot's for SERVER_NAME, once it has one, with a self-signed stand-in until
#                then; nginx takes each new one, renewals included, within a minute
set -eu

tls=/etc/chainv4/tls
mode=${TLS_MODE:-self-signed}
name=${SERVER_NAME:-localhost}
mkdir -p "$tls"

# A certificate for this name alone, by IP address or DNS name, as it is. It is no CA, which
# rustls, and so chainv4 send --ca-cert, refuses to take as a server's own certificate.
self_sign() {
    case "$name" in
        *[!0-9.]*) alt="DNS:$name" ;;
        *) alt="IP:$name" ;;
    esac

    openssl req -x509 -newkey rsa:2048 -nodes -days 825 -subj "/CN=$name" \
        -addext "subjectAltName=$alt" -addext "basicConstraints=critical,CA:FALSE" \
        -addext "extendedKeyUsage=serverAuth" -keyout "$tls/privkey.pem" -out "$tls/fullchain.pem" 2> /dev/null
    echo "[tls] made a self-signed certificate for $name"
}

case "$mode" in
    self-signed)
        # Made again when it is missing, about to expire, or made by an older tls.sh as a CA.
        if [ ! -s "$tls/fullchain.pem" ] || ! openssl x509 -in "$tls/fullchain.pem" -noout -checkend 86400 > /dev/null \
            || openssl x509 -in "$tls/fullchain.pem" -noout -text | grep -q 'CA:TRUE'; then
            self_sign
        fi
        ;;
    provided)
        provided=/etc/chainv4/provided

        if [ ! -s "$provided/fullchain.pem" ] || [ ! -s "$provided/privkey.pem" ]; then
            echo "[tls] TLS_MODE=provided needs fullchain.pem and privkey.pem in docker/certs" >&2
            exit 1
        fi

        cp "$provided/fullchain.pem" "$provided/privkey.pem" "$tls/"
        echo "[tls] using the certificate in docker/certs"
        ;;
    letsencrypt)
        live=/etc/letsencrypt/live/$name

        if [ -s "$live/fullchain.pem" ]; then
            cp "$live/fullchain.pem" "$live/privkey.pem" "$tls/"
            echo "[tls] using Let's Encrypt's certificate for $name"
        else
            self_sign
            echo "[tls] standing in until certbot has one from Let's Encrypt"
        fi

        # Each new certificate certbot gets, nginx takes, by itself.
        (
            while sleep 60; do
                if [ -s "$live/fullchain.pem" ] && ! cmp -s "$live/fullchain.pem" "$tls/fullchain.pem"; then
                    cp "$live/fullchain.pem" "$live/privkey.pem" "$tls/"
                    nginx -s reload && echo "[tls] took Let's Encrypt's new certificate for $name"
                fi
            done
        ) &
        ;;
    *)
        echo "[tls] TLS_MODE is self-signed, provided or letsencrypt, not $mode" >&2
        exit 1
        ;;
esac

# Strict-Transport-Security only for certificates browsers trust: a self-signed one would lock
# visitors out once it changed.
case "$mode" in
    self-signed) HSTS= ;;
    *) HSTS='add_header Strict-Transport-Security "max-age=31536000" always;' ;;
esac

case "${HTTPS_PORT:-443}" in
    443) HTTPS_PORT_SUFFIX= ;;
    *) HTTPS_PORT_SUFFIX=":$HTTPS_PORT" ;;
esac

# Requests a second, by address: the apps' own bursts fit well inside these, a flood does not. The
# onion's is shared by everyone on it. RATE_LIMIT=off turns them all off.
RATE_API=${RATE_API:-30} RATE_CALLS=${RATE_CALLS:-5} RATE_RANGE=${RATE_RANGE:-5}

case "$RATE_API$RATE_CALLS$RATE_RANGE" in
    *[!0-9]*) echo "RATE_API, RATE_CALLS and RATE_RANGE are whole requests a second" >&2; exit 1 ;;
esac

[ "$RATE_API" -gt 0 ] && [ "$RATE_CALLS" -gt 0 ] && [ "$RATE_RANGE" -gt 0 ] || { echo "RATE_API, RATE_CALLS and RATE_RANGE are above 0; RATE_LIMIT=off turns limits off" >&2; exit 1; }
RATE_ONION=$((RATE_API * 3)) BURST_API=$((RATE_API * 6)) BURST_CALLS=$((RATE_CALLS * 4)) BURST_RANGE=$((RATE_RANGE * 4)) BURST_ONION=$((RATE_API * 12))

case "${RATE_LIMIT:-on}" in
    off) RATE_KEY='""' RATE_ONION_KEY='""' ;;
    on) RATE_KEY='$binary_remote_addr' RATE_ONION_KEY=onion ;;
    *) echo "RATE_LIMIT is on or off" >&2; exit 1 ;;
esac

export SERVER_NAME="$name" NODE_UPSTREAM="${NODE_UPSTREAM:-node:8080}" HSTS HTTPS_PORT_SUFFIX
export RATE_API RATE_CALLS RATE_RANGE RATE_ONION BURST_API BURST_CALLS BURST_RANGE BURST_ONION RATE_KEY RATE_ONION_KEY
# shellcheck disable=SC2016
envsubst '${SERVER_NAME} ${NODE_UPSTREAM} ${HSTS} ${HTTPS_PORT_SUFFIX} ${RATE_API} ${RATE_CALLS} ${RATE_RANGE} ${RATE_ONION} ${RATE_KEY} ${RATE_ONION_KEY}' \
    < /etc/nginx/chainv4.conf.template > /etc/nginx/conf.d/default.conf
# shellcheck disable=SC2016
envsubst '${BURST_API} ${BURST_CALLS} ${BURST_RANGE} ${BURST_ONION}' \
    < /etc/nginx/chainv4-locations.conf.template > /etc/nginx/chainv4-locations.conf
